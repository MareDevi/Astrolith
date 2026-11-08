//! Signed update manifests and the capability that decides whether updates exist.
//!
//! The application ships without a manifest URL or public key, so update checks
//! are disabled until a build injects both at compile time. A fetched envelope
//! is only trusted after its Ed25519 signature verifies against that compiled-in
//! key; nothing in the payload can introduce a key. Network work stays on the
//! application service lane so rendering never blocks.
//!
//! The update check offers an external release download only. Astrolith never
//! replaces its own binary.

use std::io::Read;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::Deserialize;
use url::Url;

/// Artifact platform this Linux build can download.
pub(crate) const PLATFORM: &str = "linux-x86_64-appimage";
/// Hard cap for the manifest envelope, checked both while reading and after.
pub(crate) const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
/// Whole-request deadline for the manifest fetch.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// A version component above this is not a plausible Astrolith release.
const MAX_VERSION_COMPONENT: u64 = 9_999;

/// Why this build cannot check for updates.
///
/// Typed so the explanatory sentence is translated when the dialog renders,
/// not when the capability is detected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisabledReason {
    /// The packaging pipeline baked in no manifest URL or public key.
    NotConfigured,
    /// The configured manifest URL was not HTTPS.
    InsecureManifestUrl,
    /// The configured public key was not one 32-byte Ed25519 key.
    InvalidPublicKey,
}

impl DisabledReason {
    /// One user-facing sentence explaining why checks cannot run.
    fn message(self) -> String {
        tr!(match self {
            Self::NotConfigured => "update.capability.not_configured",
            Self::InsecureManifestUrl => "update.capability.insecure_manifest_url",
            Self::InvalidPublicKey => "update.capability.invalid_public_key",
        })
    }
}

/// Compile-time update configuration.
///
/// `ASTROLITH_UPDATE_MANIFEST_URL` and `ASTROLITH_UPDATE_PUBLIC_KEY` are baked
/// into release builds by the packaging pipeline. A local development build has
/// neither, which is the intended capability-disabled state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CapabilityState {
    /// Both values are valid; checks may run.
    Enabled(Capability),
    /// The build cannot check for updates, with the reason to show the user.
    Disabled(DisabledReason),
}

/// One validated manifest URL and its pinned Ed25519 public key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Capability {
    manifest_url: String,
    public_key: Vec<u8>,
}

impl CapabilityState {
    /// Read the capability baked into this binary.
    pub(crate) fn detect() -> Self {
        Self::from_parts(
            option_env!("ASTROLITH_UPDATE_MANIFEST_URL"),
            option_env!("ASTROLITH_UPDATE_PUBLIC_KEY"),
        )
    }

    /// Decide the capability from raw configuration values.
    pub(crate) fn from_parts(manifest_url: Option<&str>, public_key: Option<&str>) -> Self {
        let (Some(manifest_url), Some(public_key)) = (manifest_url, public_key) else {
            return Self::Disabled(DisabledReason::NotConfigured);
        };
        let manifest_url = manifest_url.trim();
        let public_key = public_key.trim();
        if !is_https(manifest_url) {
            return Self::Disabled(DisabledReason::InsecureManifestUrl);
        }
        let Ok(public_key) = STANDARD.decode(public_key) else {
            return Self::Disabled(DisabledReason::InvalidPublicKey);
        };
        if public_key.len() != 32 {
            return Self::Disabled(DisabledReason::InvalidPublicKey);
        }
        Self::Enabled(Capability {
            manifest_url: manifest_url.to_owned(),
            public_key,
        })
    }

    /// Whether checks can run at all.
    pub(crate) fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }
}

/// A numeric `major.minor.patch` release version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version {
    pub(crate) major: u64,
    pub(crate) minor: u64,
    pub(crate) patch: u64,
}

impl Version {
    /// Parse the exact `x.y.z` form used by Cargo and release tags.
    pub(crate) fn parse(text: &str) -> Result<Self, UpdateError> {
        let mut parts = text.split('.');
        let mut next = || -> Result<u64, UpdateError> {
            let part = parts
                .next()
                .ok_or_else(|| UpdateError::UnsupportedVersion(text.to_owned()))?;
            if part.is_empty() || part.len() > 1 && part.starts_with('0') {
                return Err(UpdateError::UnsupportedVersion(text.to_owned()));
            }
            let value = part
                .parse::<u64>()
                .map_err(|_| UpdateError::UnsupportedVersion(text.to_owned()))?;
            if value > MAX_VERSION_COMPONENT {
                return Err(UpdateError::UnsupportedVersion(text.to_owned()));
            }
            Ok(value)
        };
        let version = Self {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        if parts.next().is_some() {
            return Err(UpdateError::UnsupportedVersion(text.to_owned()));
        }
        Ok(version)
    }

    /// Whether this manifest version is newer than the running build.
    pub(crate) fn is_newer_than(self, running: Self) -> bool {
        self > running
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// One verified release with an artifact for this platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Release {
    /// Version the manifest offers.
    pub(crate) version: Version,
    /// Release notes shown in the dialog.
    pub(crate) notes: String,
    /// HTTPS release page, opened externally.
    pub(crate) page: String,
    /// HTTPS artifact download for [`PLATFORM`].
    pub(crate) artifact_url: String,
    /// Lowercase hex SHA-256 of the artifact.
    pub(crate) sha256: String,
}

/// What a successful check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CheckOutcome {
    /// A newer verified release is available.
    Available(Release),
    /// The running version is current (or the manifest is older).
    UpToDate {
        /// Newest version the manifest declares.
        latest: Version,
    },
}

/// User-visible result of the most recent check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// This build cannot check for updates.
    Disabled(DisabledReason),
    /// No check has finished yet.
    Idle,
    /// A check is in flight.
    Checking,
    /// The running version is current.
    UpToDate(Version),
    /// A newer release is available.
    Available(Release),
    /// A newer release exists but the user skipped that exact version.
    Skipped(Release),
    /// The last check failed with a recoverable error.
    Failed(UpdateError),
}

impl Status {
    /// The initial status for a capability.
    pub(crate) fn initial(capability: &CapabilityState) -> Self {
        match capability {
            CapabilityState::Enabled(_) => Self::Idle,
            CapabilityState::Disabled(reason) => Self::Disabled(*reason),
        }
    }

    /// Turn one check result into the status, honouring a persisted skip.
    pub(crate) fn finish(result: Result<CheckOutcome, UpdateError>, skipped: Option<&str>) -> Self {
        match result {
            Ok(CheckOutcome::UpToDate { latest }) => Self::UpToDate(latest),
            Ok(CheckOutcome::Available(release)) => {
                if skipped.and_then(|text| Version::parse(text.trim()).ok())
                    == Some(release.version)
                {
                    Self::Skipped(release)
                } else {
                    Self::Available(release)
                }
            }
            Err(error) => Self::Failed(error),
        }
    }

    /// One user-facing sentence for palette status and dialogs.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Disabled(reason) => reason.message(),
            Self::Idle => tr!("update.status.idle"),
            Self::Checking => tr!("update.status.checking"),
            Self::UpToDate(latest) => tr!("update.status.up_to_date", version = latest),
            Self::Available(release) => tr!(
                "update.status.available",
                version = release.version,
                current = env!("CARGO_PKG_VERSION")
            ),
            Self::Skipped(release) => tr!("update.status.skipped", version = release.version),
            Self::Failed(reason) => tr!("update.status.failed", reason = reason),
        }
    }

    /// The release this status describes, if any.
    pub(crate) fn release(&self) -> Option<&Release> {
        match self {
            Self::Available(release) | Self::Skipped(release) => Some(release),
            _ => None,
        }
    }
}

/// Which manifest URL was not HTTPS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UrlKind {
    /// The manifest URL baked into the build.
    Manifest,
    /// The release page the dialog opens externally.
    ReleasePage,
    /// The artifact download URL.
    Artifact,
}

/// Why a check could not produce a usable manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UpdateError {
    /// The transport itself failed.
    Network(String),
    /// The server answered with a non-success HTTP status.
    HttpStatus(u16),
    /// The response exceeded [`MAX_MANIFEST_BYTES`].
    TooLarge,
    /// JSON, base64, version or digest syntax was invalid.
    Malformed(String),
    /// A base64 field was not a valid encoding.
    InvalidEncoding(String),
    /// The signature did not verify against the compiled-in key.
    Signature,
    /// A manifest, release page or artifact URL was not HTTPS.
    InsecureUrl(UrlKind),
    /// The version text was not a plausible `x.y.z` release.
    UnsupportedVersion(String),
    /// No artifact matched [`PLATFORM`].
    NoArtifact,
    /// The artifact digest was not a 64-character SHA-256 hex string.
    InvalidDigest,
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Network(reason) => tr!("update.error.network", reason = reason),
            Self::HttpStatus(status) => tr!("update.error.http_status", status = status),
            Self::TooLarge => tr!("update.error.too_large", bytes = MAX_MANIFEST_BYTES),
            Self::Malformed(reason) => tr!("update.error.malformed", reason = reason),
            Self::InvalidEncoding(reason) => {
                tr!("update.error.invalid_encoding", reason = reason)
            }
            Self::Signature => tr!("update.error.signature"),
            Self::InsecureUrl(kind) => tr!(match kind {
                UrlKind::Manifest => "update.error.insecure_manifest_url",
                UrlKind::ReleasePage => "update.error.insecure_release_page_url",
                UrlKind::Artifact => "update.error.insecure_artifact_url",
            }),
            Self::UnsupportedVersion(version) => {
                tr!("update.error.unsupported_version", version = version)
            }
            Self::NoArtifact => tr!("update.error.no_artifact"),
            Self::InvalidDigest => tr!("update.error.invalid_digest"),
        };
        formatter.write_str(&message)
    }
}

impl std::error::Error for UpdateError {}

/// Bytes fetched from a manifest URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FetchError {
    /// The transport failed before a complete body was read.
    Network(String),
    /// The body crossed the manifest cap.
    TooLarge,
    /// The server answered with a non-success status.
    Status(u16),
    /// The URL was not HTTPS.
    Insecure,
}

/// Injectable manifest transport; tests never touch the network.
pub(crate) trait Transport {
    /// Fetch the manifest body, refusing anything above `limit` bytes.
    fn fetch(&self, url: &str, limit: usize) -> Result<Vec<u8>, FetchError>;
}

/// HTTPS transport backed by the pinned `ureq` rustls client.
pub(crate) struct HttpTransport;

impl Transport for HttpTransport {
    fn fetch(&self, url: &str, limit: usize) -> Result<Vec<u8>, FetchError> {
        if !is_https(url) {
            return Err(FetchError::Insecure);
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .max_redirects(2)
            .https_only(true)
            .http_status_as_error(false)
            .build()
            .into();
        let mut response = agent
            .get(url)
            .call()
            .map_err(|error| FetchError::Network(error.to_string()))?;
        if !response.status().is_success() {
            return Err(FetchError::Status(response.status().as_u16()));
        }
        let body = response.body_mut();
        if body
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(FetchError::TooLarge);
        }
        let mut reader = body
            .with_config()
            .limit(limit.saturating_add(1) as u64)
            .reader();
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let read = reader
                .read(&mut chunk)
                .map_err(|error| FetchError::Network(error.to_string()))?;
            if read == 0 {
                break;
            }
            if bytes.len() + read > limit {
                return Err(FetchError::TooLarge);
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
        Ok(bytes)
    }
}

/// Manifest envelope: a base64 payload and its base64 Ed25519 signature.
#[derive(Deserialize)]
struct Envelope {
    payload: String,
    signature: String,
}

/// Signed payload describing one release.
#[derive(Deserialize)]
struct ManifestPayload {
    version: String,
    #[serde(default)]
    notes: String,
    page: String,
    artifacts: Vec<ManifestArtifact>,
}

/// One platform artifact declared by the payload.
#[derive(Deserialize)]
struct ManifestArtifact {
    platform: String,
    url: String,
    sha256: String,
}

/// Fetch, verify and interpret one update manifest.
///
/// The signature covers the decoded payload bytes exactly, so any whitespace or
/// key reordering in transit invalidates the envelope. Every failure is a
/// recoverable status, never a trusted partial result.
pub(crate) fn check(
    transport: &dyn Transport,
    capability: &Capability,
    running: Version,
) -> Result<CheckOutcome, UpdateError> {
    if !is_https(&capability.manifest_url) {
        return Err(UpdateError::InsecureUrl(UrlKind::Manifest));
    }
    let bytes = transport
        .fetch(&capability.manifest_url, MAX_MANIFEST_BYTES)
        .map_err(|error| match error {
            FetchError::Network(reason) => UpdateError::Network(reason),
            FetchError::TooLarge => UpdateError::TooLarge,
            FetchError::Status(status) => UpdateError::HttpStatus(status),
            FetchError::Insecure => UpdateError::InsecureUrl(UrlKind::Manifest),
        })?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(UpdateError::TooLarge);
    }
    let envelope: Envelope = serde_json::from_slice(&bytes)
        .map_err(|error| UpdateError::Malformed(error.to_string()))?;
    let payload_bytes = decode_base64(&envelope.payload)?;
    if payload_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(UpdateError::TooLarge);
    }
    let signature = decode_base64(&envelope.signature)?;
    if signature.len() != 64 {
        return Err(UpdateError::Signature);
    }
    UnparsedPublicKey::new(&ED25519, &capability.public_key)
        .verify(&payload_bytes, &signature)
        .map_err(|_| UpdateError::Signature)?;
    let payload: ManifestPayload = serde_json::from_slice(&payload_bytes)
        .map_err(|error| UpdateError::Malformed(error.to_string()))?;
    let version = Version::parse(&payload.version)?;
    if !is_https(&payload.page) {
        return Err(UpdateError::InsecureUrl(UrlKind::ReleasePage));
    }
    let artifact = payload
        .artifacts
        .iter()
        .find(|artifact| artifact.platform == PLATFORM)
        .ok_or(UpdateError::NoArtifact)?;
    if !is_https(&artifact.url) {
        return Err(UpdateError::InsecureUrl(UrlKind::Artifact));
    }
    let sha256 = artifact.sha256.trim().to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(UpdateError::InvalidDigest);
    }
    let release = Release {
        version,
        notes: payload.notes,
        page: payload.page,
        artifact_url: artifact.url.clone(),
        sha256,
    };
    if version.is_newer_than(running) {
        Ok(CheckOutcome::Available(release))
    } else {
        Ok(CheckOutcome::UpToDate { latest: version })
    }
}

fn decode_base64(text: &str) -> Result<Vec<u8>, UpdateError> {
    STANDARD
        .decode(text.trim())
        .map_err(|error| UpdateError::InvalidEncoding(error.to_string()))
}

fn is_https(text: &str) -> bool {
    Url::parse(text).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some_and(|host| !host.is_empty())
            && url.username().is_empty()
            && url.password().is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    struct StaticTransport {
        body: Vec<u8>,
    }

    impl Transport for StaticTransport {
        fn fetch(&self, _url: &str, _limit: usize) -> Result<Vec<u8>, FetchError> {
            Ok(self.body.clone())
        }
    }

    struct SigningKey {
        pair: Ed25519KeyPair,
        public: Vec<u8>,
    }

    fn key_pair() -> SigningKey {
        let document =
            Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("generate Ed25519 key");
        let pair = Ed25519KeyPair::from_pkcs8(document.as_ref()).expect("load Ed25519 key");
        let public = pair.public_key().as_ref().to_vec();
        SigningKey { pair, public }
    }

    fn capability(public: &[u8]) -> Capability {
        let encoded = STANDARD.encode(public);
        match CapabilityState::from_parts(
            Some("https://updates.example.test/astrolith/manifest.json"),
            Some(&encoded),
        ) {
            CapabilityState::Enabled(capability) => capability,
            CapabilityState::Disabled(reason) => panic!("capability disabled: {reason:?}"),
        }
    }

    fn payload_json(version: &str) -> Vec<u8> {
        format!(
            r#"{{"version":"{version}","notes":"Fixes things","page":"https://example.test/releases/{version}","artifacts":[{{"platform":"{PLATFORM}","url":"https://example.test/astrolith-{version}.AppImage","sha256":"{}"}}]}}"#,
            "a".repeat(64)
        )
        .into_bytes()
    }

    fn envelope(payload: &[u8], pair: &Ed25519KeyPair) -> Vec<u8> {
        let signature = pair.sign(payload);
        serde_json::json!({
            "payload": STANDARD.encode(payload),
            "signature": STANDARD.encode(signature.as_ref()),
        })
        .to_string()
        .into_bytes()
    }

    fn running(version: &str) -> Version {
        Version::parse(version).unwrap()
    }

    #[test]
    fn verified_newer_release_is_offered() {
        let keys = key_pair();
        let transport = StaticTransport {
            body: envelope(&payload_json("0.2.0"), &keys.pair),
        };
        let outcome = check(&transport, &capability(&keys.public), running("0.1.0")).unwrap();
        let CheckOutcome::Available(release) = outcome else {
            panic!("expected an available release");
        };
        assert_eq!(release.version, running("0.2.0"));
        assert_eq!(release.notes, "Fixes things");
        assert_eq!(release.page, "https://example.test/releases/0.2.0");
        assert_eq!(
            release.artifact_url,
            "https://example.test/astrolith-0.2.0.AppImage"
        );
        assert_eq!(release.sha256, "a".repeat(64));
    }

    #[test]
    fn equal_and_older_versions_are_not_offered() {
        let keys = key_pair();
        let capability = capability(&keys.public);
        for (manifest, current) in [("0.2.0", "0.2.0"), ("0.1.9", "0.2.0")] {
            let transport = StaticTransport {
                body: envelope(&payload_json(manifest), &keys.pair),
            };
            assert_eq!(
                check(&transport, &capability, running(current)).unwrap(),
                CheckOutcome::UpToDate {
                    latest: running(manifest)
                }
            );
        }
    }

    #[test]
    fn tampered_payload_byte_or_wrong_key_fails_verification() {
        let keys = key_pair();
        let capability = capability(&keys.public);
        let mut payload = payload_json("0.3.0");
        let signature = keys.pair.sign(&payload);
        let position = payload.len() / 2;
        payload[position] ^= 1;
        let tampered = StaticTransport {
            body: serde_json::json!({
                "payload": STANDARD.encode(&payload),
                "signature": STANDARD.encode(signature.as_ref()),
            })
            .to_string()
            .into_bytes(),
        };
        assert_eq!(
            check(&tampered, &capability, running("0.1.0")).unwrap_err(),
            UpdateError::Signature
        );

        let other = key_pair();
        let wrong_key = StaticTransport {
            body: envelope(&payload_json("0.3.0"), &other.pair),
        };
        assert_eq!(
            check(&wrong_key, &capability, running("0.1.0")).unwrap_err(),
            UpdateError::Signature
        );
    }

    #[test]
    fn absurd_and_malformed_versions_are_rejected() {
        for version in [
            "999999.0.0",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "1.02.3",
            "1.2.x",
            "",
        ] {
            assert!(
                matches!(
                    Version::parse(version),
                    Err(UpdateError::UnsupportedVersion(_))
                ),
                "{version} must be rejected"
            );
        }
        let keys = key_pair();
        let transport = StaticTransport {
            body: envelope(&payload_json("999999.0.0"), &keys.pair),
        };
        assert!(matches!(
            check(&transport, &capability(&keys.public), running("0.1.0")),
            Err(UpdateError::UnsupportedVersion(_))
        ));
    }

    #[test]
    fn malformed_envelopes_and_digests_are_rejected() {
        let keys = key_pair();
        let capability = capability(&keys.public);
        let cases = [
            (b"not json".to_vec(), "envelope"),
            (
                serde_json::json!({"payload": "!!!", "signature": "AAAA"})
                    .to_string()
                    .into_bytes(),
                "payload base64",
            ),
            (
                serde_json::json!({"payload": "", "signature": "!!!"})
                    .to_string()
                    .into_bytes(),
                "signature base64",
            ),
            (
                serde_json::json!({"payload": "", "signature": ""})
                    .to_string()
                    .into_bytes(),
                "signature length",
            ),
        ];
        for (body, label) in cases {
            let error =
                check(&StaticTransport { body }, &capability, running("0.1.0")).unwrap_err();
            assert!(
                matches!(
                    error,
                    UpdateError::Malformed(_)
                        | UpdateError::InvalidEncoding(_)
                        | UpdateError::Signature
                ),
                "{label}: {error}"
            );
        }

        let mut payload: serde_json::Value =
            serde_json::from_slice(&payload_json("0.2.0")).unwrap();
        payload["artifacts"][0]["sha256"] = serde_json::json!("xyz");
        let body = envelope(&serde_json::to_vec(&payload).unwrap(), &keys.pair);
        assert!(matches!(
            check(&StaticTransport { body }, &capability, running("0.1.0")),
            Err(UpdateError::InvalidDigest)
        ));
    }

    #[test]
    fn insecure_urls_are_rejected_everywhere() {
        let keys = key_pair();
        let capability = capability(&keys.public);
        for mutate in [
            |payload: &mut serde_json::Value| {
                payload["page"] = serde_json::json!("http://example.test/release");
            },
            |payload: &mut serde_json::Value| {
                payload["artifacts"][0]["url"] =
                    serde_json::json!("http://example.test/a.AppImage");
            },
            |payload: &mut serde_json::Value| {
                payload["page"] = serde_json::json!("not a url");
            },
        ] {
            let mut payload: serde_json::Value =
                serde_json::from_slice(&payload_json("0.2.0")).unwrap();
            mutate(&mut payload);
            let body = envelope(&serde_json::to_vec(&payload).unwrap(), &keys.pair);
            assert!(matches!(
                check(&StaticTransport { body }, &capability, running("0.1.0")),
                Err(UpdateError::InsecureUrl(_))
            ));
        }
        let encoded = STANDARD.encode(&keys.public);
        assert!(
            !CapabilityState::from_parts(
                Some("http://updates.example.test/manifest.json"),
                Some(&encoded)
            )
            .is_enabled()
        );
    }

    #[test]
    fn missing_artifacts_oversized_bodies_and_network_failures_are_recoverable() {
        let keys = key_pair();
        let capability = capability(&keys.public);
        let mut payload: serde_json::Value =
            serde_json::from_slice(&payload_json("0.2.0")).unwrap();
        payload["artifacts"][0]["platform"] = serde_json::json!("darwin-aarch64");
        let body = envelope(&serde_json::to_vec(&payload).unwrap(), &keys.pair);
        assert_eq!(
            check(&StaticTransport { body }, &capability, running("0.1.0")).unwrap_err(),
            UpdateError::NoArtifact
        );

        let oversized = StaticTransport {
            body: vec![b' '; MAX_MANIFEST_BYTES + 1],
        };
        assert_eq!(
            check(&oversized, &capability, running("0.1.0")).unwrap_err(),
            UpdateError::TooLarge
        );

        struct Offline;
        impl Transport for Offline {
            fn fetch(&self, _url: &str, _limit: usize) -> Result<Vec<u8>, FetchError> {
                Err(FetchError::Network("connection refused".to_owned()))
            }
        }
        assert!(matches!(
            check(&Offline, &capability, running("0.1.0")),
            Err(UpdateError::Network(_))
        ));
    }

    #[test]
    fn capability_requires_both_valid_configuration_values() {
        assert!(matches!(
            CapabilityState::from_parts(None, Some("AAAA")),
            CapabilityState::Disabled(DisabledReason::NotConfigured)
        ));
        let encoded = STANDARD.encode([0u8; 32]);
        assert!(
            CapabilityState::from_parts(
                Some("https://updates.example.test/manifest.json"),
                Some(&encoded)
            )
            .is_enabled()
        );
        for key in ["not base64!", &STANDARD.encode([0u8; 31])] {
            assert!(matches!(
                CapabilityState::from_parts(
                    Some("https://updates.example.test/manifest.json"),
                    Some(key)
                ),
                CapabilityState::Disabled(DisabledReason::InvalidPublicKey)
            ));
        }
        assert!(matches!(
            CapabilityState::from_parts(None, None),
            CapabilityState::Disabled(_)
        ));
        assert!(!CapabilityState::from_parts(None, None).is_enabled());
    }

    #[test]
    fn skipped_releases_and_status_messages_stay_user_visible() {
        let keys = key_pair();
        let transport = StaticTransport {
            body: envelope(&payload_json("0.2.0"), &keys.pair),
        };
        let outcome = check(&transport, &capability(&keys.public), running("0.1.0")).unwrap();
        let skipped = Status::finish(Ok(outcome.clone()), Some("0.2.0"));
        assert!(matches!(skipped, Status::Skipped(_)));
        assert!(skipped.message().contains("skip"));
        assert!(matches!(
            Status::finish(Ok(outcome.clone()), Some("0.3.0")),
            Status::Available(_)
        ));
        assert!(matches!(
            Status::finish(Ok(outcome), Some("not a version")),
            Status::Available(_)
        ));
        assert!(
            Status::Disabled(DisabledReason::NotConfigured)
                .message()
                .contains("not configured")
        );
        assert!(
            Status::finish(Err(UpdateError::Signature), None)
                .message()
                .contains("signature")
        );
        assert_eq!(
            Status::initial(&CapabilityState::Disabled(DisabledReason::NotConfigured)),
            Status::Disabled(DisabledReason::NotConfigured)
        );
    }
}
