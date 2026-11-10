//! S3-compatible image uploads on the blocking service lane.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use astrolith_core::preferences::ImageUploadPreferences;
use astrolith_workspace::assets::CopiedAsset;
use gpui_kit::{App, AppContext, Task};
use ring::{digest, hmac};
use serde::{Deserialize, Serialize};
use url::{Host, Position, Url};

const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
type UploadOutcome = (Vec<(String, String)>, usize);

const KEYCHAIN_ACCOUNT: &str = "astrolith-s3";
const SIGNED_HEADERS: &str = "host;x-amz-content-sha256;x-amz-date";

pub(crate) fn endpoint_url(value: &str) -> Option<Url> {
    let url = web_url(value)?;
    let loopback = match url.host()? {
        Host::Domain(domain) => domain == "localhost",
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    };
    (url.path() == "/" && (url.scheme() == "https" || loopback)).then_some(url)
}

pub(crate) fn web_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

pub(crate) fn valid_bucket(value: &str) -> bool {
    let alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    (3..=63).contains(&value.len())
        && value.bytes().next().is_some_and(alphanumeric)
        && value.bytes().last().is_some_and(alphanumeric)
        && value
            .bytes()
            .all(|byte| alphanumeric(byte) || matches!(byte, b'-' | b'.'))
        && !value.contains("..")
}

pub(crate) fn valid_region(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Credentials {
    access_key: String,
    secret_key: String,
}

impl Credentials {
    pub(crate) fn new(access_key: String, secret_key: String) -> Self {
        Self {
            access_key,
            secret_key,
        }
    }
}

// The endpoint and bucket scope prevent an edit to the destination from
// forwarding saved credentials to another server or bucket.
fn credential_key(settings: &ImageUploadPreferences) -> Option<String> {
    let endpoint = endpoint_url(&settings.endpoint)?;
    valid_bucket(&settings.bucket)
        .then(|| format!("astrolith-image-upload:{endpoint}{}", settings.bucket))
}

pub(crate) fn save_credentials(
    settings: &ImageUploadPreferences,
    credentials: Credentials,
    cx: &App,
) -> Task<anyhow::Result<()>> {
    let Some(key) = credential_key(settings) else {
        return Task::ready(Err(anyhow::anyhow!("invalid image upload destination")));
    };
    match serde_json::to_vec(&credentials) {
        Ok(bytes) => cx.write_credentials(&key, KEYCHAIN_ACCOUNT, &bytes),
        Err(error) => Task::ready(Err(error.into())),
    }
}

pub(crate) fn load_credentials(
    settings: &ImageUploadPreferences,
    cx: &App,
) -> Task<anyhow::Result<Option<Credentials>>> {
    let Some(key) = credential_key(settings) else {
        return Task::ready(Ok(None));
    };
    let read = cx.read_credentials(&key);
    cx.background_spawn(async move {
        let Some((_, bytes)) = read.await? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_slice(&bytes)?))
    })
}

pub(crate) fn start_upload(
    settings: ImageUploadPreferences,
    root: PathBuf,
    assets: Vec<CopiedAsset>,
    cx: &App,
) -> Task<Result<UploadOutcome, String>> {
    let read = load_credentials(&settings, cx);
    cx.spawn(async move |cx| {
        let credentials = match read.await {
            Ok(Some(credentials)) => credentials,
            Ok(None) => return Err(tr!("editor.clipboard.upload_credentials_missing")),
            Err(_) => return Err(tr!("editor.clipboard.upload_keychain_unavailable")),
        };
        let upload = cx.update(|cx| {
            crate::runtime::blocking(cx, move || {
                upload_images(&settings, credentials, &root, &assets)
            })
        });
        upload
            .await
            .map_err(|_| tr!("editor.clipboard.upload_failed"))
    })
}

/// Successful uploads retain their original Markdown for the guarded editor patch.
fn upload_images(
    settings: &ImageUploadPreferences,
    credentials: Credentials,
    root: &Path,
    assets: &[CopiedAsset],
) -> UploadOutcome {
    let images = assets.iter().filter(|asset| asset.is_image());
    let Some(target) = Target::from_preferences(settings, credentials) else {
        return (Vec::new(), images.count());
    };
    let mut replacements = Vec::new();
    let mut failed = 0;
    for image in images {
        if let Some(markdown) = target.upload(root, image) {
            replacements.push((image.markdown().to_owned(), markdown));
        } else {
            failed += 1;
        }
    }
    (replacements, failed)
}

struct Target {
    endpoint: Url,
    public: Url,
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    agent: ureq::Agent,
}

impl Target {
    fn from_preferences(
        settings: &ImageUploadPreferences,
        credentials: Credentials,
    ) -> Option<Self> {
        let endpoint = endpoint_url(&settings.endpoint)?;
        let public = web_url(&settings.public_url)?;
        if !valid_bucket(&settings.bucket) || !valid_region(&settings.region) {
            return None;
        }
        Some(Self {
            endpoint,
            public,
            bucket: settings.bucket.clone(),
            region: settings.region.clone(),
            access_key: credentials.access_key,
            secret_key: credentials.secret_key,
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .max_redirects(0)
                .http_status_as_error(false)
                .build()
                .into(),
        })
    }

    fn upload(&self, root: &Path, image: &CopiedAsset) -> Option<String> {
        let path = root.join(image.source()).canonicalize().ok()?;
        if !path.starts_with(root) || !path.is_file() {
            return None;
        }
        let extension = Path::new(image.source()).extension()?.to_str()?;
        let mime = match extension {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "avif" => "image/avif",
            "bmp" => "image/bmp",
            "ico" => "image/x-icon",
            "tif" | "tiff" => "image/tiff",
            "svg" => "image/svg+xml",
            "pnm" => "image/x-portable-anymap",
            _ => return None,
        };
        let mut bytes = Vec::new();
        File::open(path)
            .ok()?
            .take(MAX_IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_IMAGE_BYTES {
            return None;
        }
        let url = self.put_image(&bytes, extension, mime)?;
        Some(image.markdown_at(url.as_str()))
    }

    fn put_image(&self, bytes: &[u8], extension: &str, mime: &str) -> Option<Url> {
        let payload_hash = sha256(bytes);
        let filename = format!("{payload_hash}.{extension}");
        let mut upload = self.endpoint.clone();
        upload
            .path_segments_mut()
            .ok()?
            .pop_if_empty()
            .extend([&self.bucket, "images", &filename]);
        let mut public = self.public.clone();
        public
            .path_segments_mut()
            .ok()?
            .pop_if_empty()
            .extend(["images", &filename]);
        let host = &upload[Position::BeforeHost..Position::AfterPort];
        let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let canonical = format!(
            "PUT\n{}\n\nhost:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{timestamp}\n\n{SIGNED_HEADERS}\n{payload_hash}",
            upload.path()
        );
        let scope = format!("{}/{}/s3/aws4_request", &timestamp[..8], self.region);
        let signature = sign(
            &self.secret_key,
            &self.region,
            &timestamp,
            &scope,
            &canonical,
        );
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={SIGNED_HEADERS},Signature={signature}",
            self.access_key
        );
        let response = self
            .agent
            .put(upload.as_str())
            .header("content-type", mime)
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", timestamp)
            .header("authorization", authorization)
            .send(bytes)
            .ok()?;
        response.status().is_success().then_some(public)
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, bytes).as_ref())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn sign(secret: &str, region: &str, timestamp: &str, scope: &str, canonical: &str) -> String {
    let hmac = |key: &[u8], value: &str| {
        hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), value.as_bytes())
    };
    let date = hmac(format!("AWS4{secret}").as_bytes(), &timestamp[..8]);
    let region = hmac(date.as_ref(), region);
    let service = hmac(region.as_ref(), "s3");
    let signing_key = hmac(service.as_ref(), "aws4_request");
    let message = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        sha256(canonical.as_bytes())
    );
    hex(hmac(signing_key.as_ref(), &message).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, net::TcpListener};

    #[test]
    fn credentials_are_scoped_to_the_canonical_endpoint_and_bucket() {
        let mut settings = ImageUploadPreferences {
            endpoint: "https://s3.example.test:443/".into(),
            bucket: "images".into(),
            ..Default::default()
        };
        let key = credential_key(&settings).unwrap();
        settings.endpoint = "https://s3.example.test".into();
        assert_eq!(credential_key(&settings).as_deref(), Some(key.as_str()));
        settings.bucket = "another-bucket".into();
        assert_ne!(credential_key(&settings).as_deref(), Some(key.as_str()));
        settings.bucket = "images".into();
        settings.endpoint = "https://other.example.test".into();
        assert_ne!(credential_key(&settings).as_deref(), Some(key.as_str()));
    }

    #[test]
    fn signature_matches_aws_put_object_example() {
        let canonical = concat!(
            "PUT\n/test%24file.text\n\n",
            "date:Fri, 24 May 2013 00:00:00 GMT\n",
            "host:examplebucket.s3.amazonaws.com\n",
            "x-amz-content-sha256:44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072\n",
            "x-amz-date:20130524T000000Z\n",
            "x-amz-storage-class:REDUCED_REDUNDANCY\n\n",
            "date;host;x-amz-content-sha256;x-amz-date;x-amz-storage-class\n",
            "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072",
        );
        assert_eq!(
            sign(
                "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
                "us-east-1",
                "20130524T000000Z",
                "20130524/us-east-1/s3/aws4_request",
                canonical,
            ),
            "98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd",
        );
    }

    #[test]
    fn put_uses_signed_bucket_path_and_requires_success_before_linking() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let target = Target {
            endpoint: endpoint_url(&origin).unwrap(),
            public: web_url("https://images.example.test/cdn/").unwrap(),
            bucket: "bucket".to_owned(),
            region: "auto".to_owned(),
            access_key: "test-key".to_owned(),
            secret_key: "test-secret".to_owned(),
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(5)))
                .http_status_as_error(false)
                .max_redirects(0)
                .build()
                .into(),
        };
        let body = b"encoded image";
        let hash = sha256(body);
        let file = format!("{hash}.png");
        let expected_request = format!("PUT /bucket/images/{file} HTTP/1.1");
        let expected_public = format!("https://images.example.test/cdn/images/{file}");
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                listener.set_nonblocking(true).unwrap();
                for status in ["200 OK", "403 Forbidden"] {
                    let start = std::time::Instant::now();
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error)
                                if error.kind() == std::io::ErrorKind::WouldBlock
                                    && start.elapsed() < Duration::from_secs(5) =>
                            {
                                std::thread::sleep(Duration::from_millis(5));
                            }
                            Err(error) => panic!("accept failed: {error}"),
                        }
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = std::io::BufReader::new(&stream);
                    use std::io::BufRead as _;
                    let mut line = String::new();
                    request.read_line(&mut line).unwrap();
                    assert_eq!(line.trim_end(), expected_request);
                    let mut headers = String::new();
                    loop {
                        line.clear();
                        assert_ne!(request.read_line(&mut line).unwrap(), 0);
                        if line == "\r\n" {
                            break;
                        }
                        headers.push_str(&line.to_ascii_lowercase());
                    }
                    assert!(headers.contains("content-type: image/png"));
                    assert!(headers.contains(&format!("x-amz-content-sha256: {hash}")));
                    assert!(headers.contains("/auto/s3/aws4_request,signedheaders="));
                    let mut received = [0; 13];
                    request.read_exact(&mut received).unwrap();
                    assert_eq!(&received, body);
                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                }
            });
            assert_eq!(
                target.put_image(body, "png", "image/png").unwrap().as_str(),
                expected_public
            );
            assert!(target.put_image(body, "png", "image/png").is_none());
            worker.join().unwrap();
        });
    }
}
