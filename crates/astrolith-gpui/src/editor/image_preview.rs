//! Alt-hover image previews: resolution, bounded decoding and stale results.
//!
//! The editor cannot map a pointer to a caret, so the preview candidate is a
//! range under the pointer: the document pane inverts `range_to_bounds` over
//! the image links this module finds. Resolution is lexical and does no I/O;
//! loading runs on the service lanes, rejects anything that canonicalises
//! outside the project root, bounds both source bytes and decoded dimensions,
//! and is discarded when a newer ticket or the modifier release supersedes it.

use std::{
    fs,
    io::{Cursor, Read as _},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::RenderImage;

/// Largest image file read for a preview, in bytes.
pub(crate) const MAX_PREVIEW_BYTES: u64 = 20 * 1024 * 1024;
/// Largest accepted image extent, in pixels, on either axis.
pub(crate) const MAX_PREVIEW_DIMENSION: u32 = 16_384;
/// Deadline after which a completed preview is never shown.
pub(crate) const PREVIEW_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a preview was refused before it reached the overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PreviewError {
    /// The target is not a regular file.
    NotAFile,
    /// A directory on the preview path is not a directory.
    NotADirectory,
    /// The target resolves outside the project root.
    OutsideProject,
    /// A symlink was found on the preview path.
    Symlink,
    /// The source file is larger than [`MAX_PREVIEW_BYTES`].
    TooLarge,
    /// The decoded image exceeds the dimension or allocation limits.
    Dimensions,
    /// The bytes are not a supported raster image.
    Undecodable,
    /// A filesystem or network read failed.
    Io(String),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAFile => formatter.write_str("the image is not a regular file"),
            Self::NotADirectory => formatter.write_str("the image path is not a directory"),
            Self::OutsideProject => formatter.write_str("the image is outside the project"),
            Self::Symlink => formatter.write_str("the image path contains a symlink"),
            Self::TooLarge => formatter.write_str("the image is too large"),
            Self::Dimensions => formatter.write_str("the image dimensions are too large"),
            Self::Undecodable => formatter.write_str("the image could not be decoded"),
            Self::Io(message) => formatter.write_str(message),
        }
    }
}

/// A resolved preview candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PreviewSource {
    /// An absolute `http`/`https` URL.
    Remote(String),
    /// A lexical path already proven inside the project root.
    Local(PathBuf),
}

/// Every image link in `text`, with the range to hit-test.
///
/// The range includes the leading `!`, so Alt-hovering anywhere on the image
/// Markdown opens the preview. Destinations use Markdown's first-parenthesis
/// rule from the document scanner.
pub(crate) fn image_links(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
    astrolith_document::inline_links(text)
        .into_iter()
        .filter_map(|link| {
            let bang = link.range.start.checked_sub(1)?;
            if text.as_bytes().get(bang) != Some(&b'!') {
                return None;
            }
            let destination = preview_destination(&text[link.destination.clone()])?;
            Some((bang..link.range.end, destination))
        })
        .collect()
}

/// Clean a Markdown destination into the text the preview may resolve.
///
/// Angle brackets are removed, an unescaped title after whitespace is dropped,
/// and backslash escapes are resolved. Control characters and an empty result
/// are refused.
pub(crate) fn preview_destination(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let raw = if let Some(inner) = raw.strip_prefix('<') {
        inner.split_once('>')?.0
    } else {
        raw.split_whitespace().next().unwrap_or("")
    };
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return None;
    }
    let mut destination = String::with_capacity(raw.len());
    let mut characters = raw.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some(escaped @ ('(' | ')' | '\\' | '<' | '>')) => destination.push(escaped),
                Some(other) => {
                    destination.push('\\');
                    destination.push(other);
                }
                None => return None,
            }
        } else {
            destination.push(character);
        }
    }
    Some(destination)
}

/// Resolve one image destination without touching the filesystem.
///
/// Absolute `http`/`https` URLs become remote sources. A leading `/` names the
/// project root; anything else resolves from the document's directory. The
/// result must stay inside `root` after lexical `.`/`..` normalisation, and
/// any other URL scheme, protocol-relative path or host-absolute path yields
/// no preview at all.
pub(crate) fn resolve_preview(root: &Path, document: &Path, raw: &str) -> Option<PreviewSource> {
    let destination = preview_destination(raw)?;
    if let Some(url) = super::formatting::external_url(&destination) {
        return Some(PreviewSource::Remote(url));
    }
    if destination.contains("://")
        || destination.starts_with("//")
        || destination.contains(['\\', ':'])
    {
        return None;
    }
    let candidate = if let Some(rest) = destination.strip_prefix('/') {
        root.join(rest)
    } else {
        document.parent()?.join(&destination)
    };
    let normalized = normalize_lexical(&candidate)?;
    if !normalized.starts_with(root) {
        return None;
    }
    Some(PreviewSource::Local(normalized))
}

pub(crate) fn normalize_lexical(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    Some(normalized)
}

/// Read and decode one local preview on the blocking lane.
pub(crate) fn load_local(root: &Path, path: &Path) -> Result<Arc<RenderImage>, PreviewError> {
    let canonical_root =
        fs::canonicalize(root).map_err(|error| PreviewError::Io(error.to_string()))?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| PreviewError::OutsideProject)?;
    let mut current = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(segment) = component else {
            return Err(PreviewError::OutsideProject);
        };
        current.push(segment);
        let metadata =
            fs::symlink_metadata(&current).map_err(|error| PreviewError::Io(error.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(PreviewError::Symlink);
        }
        if components.peek().is_some() {
            if !metadata.is_dir() {
                return Err(PreviewError::NotADirectory);
            }
            continue;
        }
        if !metadata.is_file() {
            return Err(PreviewError::NotAFile);
        }
        if metadata.len() > MAX_PREVIEW_BYTES {
            return Err(PreviewError::TooLarge);
        }
    }
    let canonical = fs::canonicalize(path).map_err(|error| PreviewError::Io(error.to_string()))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(PreviewError::OutsideProject);
    }
    let bytes = fs::read(&canonical).map_err(|error| PreviewError::Io(error.to_string()))?;
    if bytes.len() as u64 > MAX_PREVIEW_BYTES {
        return Err(PreviewError::TooLarge);
    }
    decode_preview(&bytes)
}

/// Decode bounded image bytes into a renderable BGRA frame.
pub(crate) fn decode_preview(bytes: &[u8]) -> Result<Arc<RenderImage>, PreviewError> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| PreviewError::Undecodable)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PREVIEW_DIMENSION);
    limits.max_image_height = Some(MAX_PREVIEW_DIMENSION);
    limits.max_alloc = Some(MAX_PREVIEW_BYTES.saturating_mul(4));
    reader.limits(limits);
    let image = reader.decode().map_err(|error| {
        if error.to_string().contains("limit") {
            PreviewError::Dimensions
        } else {
            PreviewError::Undecodable
        }
    })?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 || width > MAX_PREVIEW_DIMENSION || height > MAX_PREVIEW_DIMENSION
    {
        return Err(PreviewError::Dimensions);
    }
    let mut rgba = image.to_rgba8().into_raw();
    // GPUI renders BGRA; the pinned loader swaps red and blue the same way.
    for pixel in 0..rgba.len() / 4 {
        rgba.swap(pixel * 4, pixel * 4 + 2);
    }
    let buffer =
        image::RgbaImage::from_raw(width, height, rgba).ok_or(PreviewError::Undecodable)?;
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// Fetch and decode one remote preview on the blocking lane.
///
/// The agent carries [`PREVIEW_TIMEOUT`] as a total request deadline, follows
/// at most five redirects and streams the body through a hard byte cap so a
/// huge response never becomes a huge allocation. `ureq` is pinned with its
/// default rustls transport; no GPUI HTTP client is required.
pub(crate) fn load_remote(url: &str) -> Result<Arc<RenderImage>, PreviewError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(PREVIEW_TIMEOUT))
        .max_redirects(5)
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|error| PreviewError::Io(error.to_string()))?;
    if !response.status().is_success() {
        return Err(PreviewError::Io(format!(
            "the image server answered {}",
            response.status()
        )));
    }
    let body = response.body_mut();
    if body
        .content_length()
        .is_some_and(|length| length > MAX_PREVIEW_BYTES)
    {
        return Err(PreviewError::TooLarge);
    }
    // The reader's own cap allows exactly one byte beyond the limit, so an
    // exact-limit body still reaches EOF while anything larger is refused.
    let mut reader = body
        .with_config()
        .limit(MAX_PREVIEW_BYTES.saturating_add(1))
        .reader();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(read) => read,
            Err(error) => {
                return Err(if is_body_limit_error(&error) {
                    PreviewError::TooLarge
                } else {
                    PreviewError::Io(error.to_string())
                });
            }
        };
        if read == 0 {
            break;
        }
        if bytes.len() + read > MAX_PREVIEW_BYTES as usize {
            return Err(PreviewError::TooLarge);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    decode_preview(&bytes)
}

fn is_body_limit_error(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<ureq::Error>())
        .is_some_and(|error| matches!(error, ureq::Error::BodyExceedsLimit(_)))
}

/// Preview overlay state: one ticket per request, one displayed image.
#[derive(Default)]
pub(crate) struct ImagePreviewState {
    generation: u64,
    image: Option<Arc<RenderImage>>,
}

impl ImagePreviewState {
    /// Hide the overlay and invalidate every in-flight request.
    pub(crate) fn dismiss(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.image = None;
    }

    /// Current ticket; every loaded result must carry it to be shown.
    pub(crate) fn begin(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.image = None;
        self.generation
    }

    /// Ticket of the newest request; a completed task may only apply this one.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether an image is currently displayed.
    pub(crate) fn is_visible(&self) -> bool {
        self.image.is_some()
    }

    /// The image to draw, if one is current.
    pub(crate) fn image(&self) -> Option<Arc<RenderImage>> {
        self.image.clone()
    }

    /// Apply one result only when its ticket and deadline are still current.
    pub(crate) fn accept(
        &mut self,
        generation: u64,
        started: Instant,
        now: Instant,
        image: Option<Arc<RenderImage>>,
    ) -> bool {
        if generation != self.generation || now.saturating_duration_since(started) > PREVIEW_TIMEOUT
        {
            return false;
        }
        self.image = image.filter(|_| self.generation == generation);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::time::Duration;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([1, 2, 3, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    /// Serve one fixed HTTP response on a loopback port.
    fn serve(response: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&response);
        });
        (format!("http://{address}/image.png"), server)
    }

    fn response(content_type: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    #[test]
    fn image_links_include_the_bang_and_follow_destination_rules() {
        let text = "![a](../img/x.png) and ![b](<my file.png> \"title\") [c](https://x.dev)";
        let links = image_links(text);
        assert_eq!(links.len(), 2);
        assert_eq!(&text[links[0].0.clone()], "![a](../img/x.png)");
        assert_eq!(links[0].1, "../img/x.png");
        assert_eq!(links[1].1, "my file.png");
    }

    #[test]
    fn destinations_strip_titles_and_unescape_parentheses() {
        assert_eq!(preview_destination("a.png \"t\"").as_deref(), Some("a.png"));
        assert_eq!(
            preview_destination("a\\(b\\).png").as_deref(),
            Some("a(b).png")
        );
        assert_eq!(preview_destination("<a b.png>").as_deref(), Some("a b.png"));
        assert_eq!(preview_destination(""), None);
        assert_eq!(preview_destination("<unterminated"), None);
    }

    #[test]
    fn resolution_covers_remote_root_and_document_relative_sources() {
        let root = Path::new("/project");
        let document = Path::new("/project/src/posts/a.md");
        assert_eq!(
            resolve_preview(root, document, "https://x.dev/a.png"),
            Some(PreviewSource::Remote("https://x.dev/a.png".into()))
        );
        assert_eq!(
            resolve_preview(root, document, "../assets/a.png"),
            Some(PreviewSource::Local(PathBuf::from(
                "/project/src/assets/a.png"
            )))
        );
        assert_eq!(
            resolve_preview(root, document, "/assets/a.png"),
            Some(PreviewSource::Local(PathBuf::from("/project/assets/a.png")))
        );
        for refused in [
            "../../../etc/passwd",
            "/../etc/passwd",
            "file:///etc/passwd",
            "data:image/png;base64,AAAA",
            "javascript:alert(1)",
            "//evil.example/a.png",
            "C:\\Windows\\a.png",
        ] {
            assert_eq!(resolve_preview(root, document, refused), None, "{refused}");
        }
    }

    #[test]
    fn local_loading_follows_nested_project_directories() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        fs::create_dir_all(root.join("src/assets")).unwrap();
        let nested = root.join("src/assets/x.png");
        fs::write(&nested, png(2, 2)).unwrap();
        let image = load_local(root, &nested).unwrap();
        assert_eq!(image.size(0).width.0, 2);

        // A file where the remaining path expects a directory is refused.
        fs::write(root.join("src/assets/blocked"), b"x").unwrap();
        assert_eq!(
            load_local(root, &root.join("src/assets/blocked/x.png")),
            Err(PreviewError::NotADirectory)
        );
    }

    #[test]
    fn local_loading_rejects_traversal_symlinks_and_oversized_files() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = project.path();
        let image = root.join("img.png");
        fs::write(&image, png(2, 2)).unwrap();
        assert!(load_local(root, &image).is_ok());

        let traversal = root.join("../outside.png");
        assert_eq!(
            load_local(root, &traversal),
            Err(PreviewError::OutsideProject)
        );

        fs::write(outside.path().join("secret.png"), png(1, 1)).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.png"), root.join("link.png"))
            .unwrap();
        assert_eq!(
            load_local(root, &root.join("link.png")),
            Err(PreviewError::Symlink)
        );

        let large = root.join("large.png");
        let file = fs::File::create(&large).unwrap();
        file.set_len(MAX_PREVIEW_BYTES + 1).unwrap();
        assert_eq!(load_local(root, &large), Err(PreviewError::TooLarge));
    }

    #[test]
    fn decoding_bounds_dimensions_and_rejects_non_images() {
        let image = decode_preview(&png(3, 4)).unwrap();
        assert_eq!(image.size(0).width.0, 3);
        assert_eq!(image.size(0).height.0, 4);
        assert_eq!(
            decode_preview(b"not an image"),
            Err(PreviewError::Undecodable)
        );
        // A header claiming an enormous canvas is refused before allocation.
        let mut oversized = png(1, 1);
        // PNG IHDR width at byte 16, height at byte 20.
        oversized[16..20].copy_from_slice(&(MAX_PREVIEW_DIMENSION + 1).to_be_bytes());
        assert!(matches!(
            decode_preview(&oversized),
            Err(PreviewError::Dimensions | PreviewError::Undecodable)
        ));
    }

    #[test]
    fn remote_loading_fetches_images_and_refuses_non_images_and_oversized_bodies() {
        let (url, server) = serve(response("image/png", &png(2, 3)));
        let image = load_remote(&url).unwrap();
        server.join().unwrap();
        assert_eq!(image.size(0).width.0, 2);
        assert_eq!(image.size(0).height.0, 3);

        let (url, server) = serve(response("text/plain", b"not an image"));
        assert_eq!(load_remote(&url), Err(PreviewError::Undecodable));
        server.join().unwrap();

        // A declared length beyond the cap is refused before the body arrives.
        let oversized = b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 20971521\r\nConnection: close\r\n\r\n".to_vec();
        let (url, server) = serve(oversized);
        assert_eq!(load_remote(&url), Err(PreviewError::TooLarge));
        server.join().unwrap();

        // A close-delimited body that grows past the cap is refused while
        // streaming, so the buffer never exceeds the limit plus one chunk.
        let mut close_delimited =
            b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nConnection: close\r\n\r\n".to_vec();
        close_delimited.extend(std::iter::repeat_n(0u8, MAX_PREVIEW_BYTES as usize + 1));
        let (url, server) = serve(close_delimited);
        assert_eq!(load_remote(&url), Err(PreviewError::TooLarge));
        server.join().unwrap();
    }

    #[test]
    fn stale_or_late_results_never_replace_the_visible_preview() {
        let mut state = ImagePreviewState::default();
        let first = state.begin();
        let second = state.begin();
        let started = Instant::now();
        let image = decode_preview(&png(1, 1)).unwrap();
        assert!(!state.accept(first, started, Instant::now(), Some(image.clone())));
        assert!(!state.is_visible());
        assert!(state.accept(second, started, Instant::now(), Some(image.clone())));
        assert!(state.is_visible());
        let third = state.begin();
        assert!(!state.is_visible());
        assert!(!state.accept(
            third,
            started,
            started + PREVIEW_TIMEOUT + Duration::from_millis(1),
            Some(image)
        ));
        assert!(!state.is_visible());
        state.dismiss();
        assert!(!state.is_visible());
    }
}
