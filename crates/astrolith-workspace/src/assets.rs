//! Asset copying, Markdown link generation and Content Finder link targets.
//!
//! Every function here is pure or performs explicit filesystem work; the GPUI
//! layer calls the copying entry points from its blocking service lane. File
//! names are sanitised, dated and collision-suffixed atomically, and every
//! destination is contained by the project root, with symlinks refused.

use std::{
    fs::{self, OpenOptions},
    io::{self},
    path::{Component, Path, PathBuf},
};

use astrolith_core::preferences::ImagePathMode;

/// Assets directory used when no scope configured one.
pub const DEFAULT_ASSETS_DIRECTORY: &str = "src/assets";
/// Maximum `-N` collision suffixes attempted by one asset copy.
pub const MAX_ASSET_ATTEMPTS: usize = 1000;
/// Extensions that insert image Markdown instead of a file link.
///
/// This is wider than preview decoding: `avif`, `tif`, `tiff`, `ico` and
/// `svg` still insert image Markdown, but the pinned `image` decoder only
/// enables `bmp`, `gif`, `jpeg`, `png` and `webp`, so their preview fails
/// silently.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "ico", "tif", "tiff", "svg", "pnm",
];

/// A refused or failed asset operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetError {
    /// The configured assets directory is outside the project root.
    OutsideProject(PathBuf),
    /// A symlink appeared where a real directory or file is required.
    Symlink(PathBuf),
    /// The configured assets path is not a directory.
    NotADirectory(PathBuf),
    /// The source is not a regular file.
    NotAFile(PathBuf),
    /// The source does not exist.
    Missing(PathBuf),
    /// The `YYYY-MM-DD` prefix is not a calendar date.
    InvalidDate(String),
    /// A name or extension has no safe characters left.
    InvalidName(String),
    /// The copy did not preserve the source byte length.
    CopyMismatch(PathBuf),
    /// Every `-N` collision suffix was taken.
    NameAttemptsExhausted,
    /// A filesystem operation failed.
    Io {
        /// Affected path.
        path: PathBuf,
        /// Operating-system message.
        message: String,
    },
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideProject(path) => {
                write!(formatter, "{} is outside the project", path.display())
            }
            Self::Symlink(path) => {
                write!(formatter, "symlinks are not allowed: {}", path.display())
            }
            Self::NotADirectory(path) => {
                write!(formatter, "{} is not a directory", path.display())
            }
            Self::NotAFile(path) => write!(formatter, "{} is not a regular file", path.display()),
            Self::Missing(path) => write!(formatter, "{} does not exist", path.display()),
            Self::InvalidDate(date) => write!(formatter, "invalid asset date {date:?}"),
            Self::InvalidName(name) => write!(formatter, "invalid asset name {name:?}"),
            Self::CopyMismatch(path) => {
                write!(
                    formatter,
                    "the asset copy at {} is not byte-identical",
                    path.display()
                )
            }
            Self::NameAttemptsExhausted => write!(
                formatter,
                "no free asset name after {MAX_ASSET_ATTEMPTS} attempts"
            ),
            Self::Io { path, message } => write!(formatter, "{}: {message}", path.display()),
        }
    }
}

impl std::error::Error for AssetError {}

/// Canonical project root plus the validated assets directory inside it.
///
/// Resolution proves the configured path is inside the project and that no
/// existing component is a symlink. The directory itself is created lazily on
/// the first copy, and every copy re-checks containment after creation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetLocation {
    root: PathBuf,
    directory: PathBuf,
}

impl AssetLocation {
    /// Resolve the effective assets directory for one project.
    ///
    /// Resolution is lexical and performs no I/O, so callers on the render
    /// path can hold the result. The project root comes from discovery and is
    /// already canonical; symlink and containment checks run again whenever a
    /// copy creates or writes inside the directory.
    pub fn resolve(root: impl AsRef<Path>, configured: Option<&str>) -> Result<Self, AssetError> {
        let root = root.as_ref().to_path_buf();
        if !root.is_absolute() {
            return Err(AssetError::OutsideProject(root));
        }
        let configured = configured
            .map(str::trim)
            .filter(|directory| !directory.is_empty())
            .unwrap_or(DEFAULT_ASSETS_DIRECTORY);
        let relative = assets_relative(&root, Path::new(configured))?;
        let directory = lexical_join(&root, &relative)?;
        Ok(Self { root, directory })
    }

    /// Canonical project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Assets directory, contained by [`Self::root`].
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

/// One dropped or picked source file to copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetCopyRequest {
    /// User-selected source file; it may live outside the project.
    pub source: PathBuf,
    /// Local `YYYY-MM-DD` name prefix.
    pub date: String,
    /// Project-relative source of the document receiving the Markdown.
    pub document: String,
    /// Relative or project-root path text to emit.
    pub path_mode: ImagePathMode,
}

/// A copied asset and the Markdown that references it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopiedAsset {
    source: String,
    path: String,
    markdown: String,
    image: bool,
    alt: String,
}

impl CopiedAsset {
    /// Project-relative source of the copied file.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Path text for the document, in the requested mode.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Image or link Markdown, already escaped and ready to insert.
    pub fn markdown(&self) -> &str {
        &self.markdown
    }

    /// Markdown referencing a different destination with the same link text.
    pub fn markdown_at(&self, destination: &str) -> String {
        markdown_for_asset(&self.alt, destination, self.image)
    }

    /// Whether the extension selected image Markdown.
    pub fn is_image(&self) -> bool {
        self.image
    }
}

/// Copy one file into the resolved assets directory, preserving its bytes.
pub fn copy_asset(
    location: &AssetLocation,
    request: &AssetCopyRequest,
) -> Result<CopiedAsset, AssetError> {
    copy_asset_bounded(location, request, MAX_ASSET_ATTEMPTS)
}

fn copy_asset_bounded(
    location: &AssetLocation,
    request: &AssetCopyRequest,
    attempts: usize,
) -> Result<CopiedAsset, AssetError> {
    let metadata = fs::symlink_metadata(&request.source).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => AssetError::Missing(request.source.clone()),
        _ => io_error(&request.source, error),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(AssetError::Symlink(request.source.clone()));
    }
    if !metadata.is_file() {
        return Err(AssetError::NotAFile(request.source.clone()));
    }
    write_asset(location, request, attempts, |target| {
        let input = fs::File::open(&request.source)
            .map_err(|error| CopyAttempt::Failed(io_error(&request.source, error)))?;
        write_new_file(input, target, metadata.len())
    })
}

/// Save encoded clipboard image bytes using the regular asset naming rules.
pub fn write_clipboard_image(
    location: &AssetLocation,
    extension: &str,
    bytes: &[u8],
    date: &str,
    document: &str,
    path_mode: ImagePathMode,
) -> Result<CopiedAsset, AssetError> {
    let request = AssetCopyRequest {
        source: PathBuf::from(format!("clipboard-image.{}", sanitize_extension(extension))),
        date: date.to_owned(),
        document: document.to_owned(),
        path_mode,
    };
    write_asset(location, &request, MAX_ASSET_ATTEMPTS, |target| {
        write_new_file(bytes, target, bytes.len() as u64)
    })
}

fn write_asset(
    location: &AssetLocation,
    request: &AssetCopyRequest,
    attempts: usize,
    mut write: impl FnMut(&Path) -> Result<(), CopyAttempt>,
) -> Result<CopiedAsset, AssetError> {
    if crate::list::parse_date(&request.date).is_none() || request.date.len() != 10 {
        return Err(AssetError::InvalidDate(request.date.clone()));
    }
    let stem = request
        .source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(sanitize_stem)
        .ok_or_else(|| AssetError::InvalidName(request.source.display().to_string()))?;
    // A name made entirely of non-ASCII or punctuation still copies; upstream
    // keeps the file and lets the dated name carry the identity.
    let stem = if stem.is_empty() {
        "file".to_owned()
    } else {
        stem
    };
    let extension = request
        .source
        .extension()
        .and_then(|extension| extension.to_str())
        .map(sanitize_extension)
        .filter(|extension| !extension.is_empty());
    let directory = prepare_directory(location)?;
    let stem = truncate_stem(&stem, extension.as_deref(), &request.date);
    let mut target = None;
    for attempt in 0..attempts {
        let name = asset_file_name(&stem, extension.as_deref(), &request.date, attempt);
        let candidate = directory.join(&name);
        match fs::symlink_metadata(&candidate) {
            Ok(existing) if existing.file_type().is_symlink() => {
                return Err(AssetError::Symlink(candidate));
            }
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(&candidate, error)),
        }
        match write(&candidate) {
            Ok(()) => {
                target = Some(candidate);
                break;
            }
            // A concurrent writer won the name between the check and the atomic
            // create; that is an ordinary collision, not a failed copy.
            Err(CopyAttempt::Collision) => continue,
            Err(CopyAttempt::Failed(error)) => return Err(error),
        }
    }
    let target = target.ok_or(AssetError::NameAttemptsExhausted)?;
    let source = project_relative_path(location.root(), &target)?;
    let path = asset_path(request.path_mode, &request.document, &source);
    let image = extension.as_deref().is_some_and(is_image_extension);
    let markdown = markdown_for_asset(&stem, &path, image);
    tracing::info!(
        from = %request.source.display(),
        to = %source,
        image,
        "copied an asset into the project"
    );
    Ok(CopiedAsset {
        source,
        path,
        markdown,
        image,
        alt: stem,
    })
}

/// Sanitize one file stem to lower-case kebab case.
///
/// Whitespace, underscores and punctuation become separators; only ASCII
/// letters and digits survive, runs collapse to one `-`, and the result never
/// starts or ends with a dash. An input with no safe characters is empty.
pub fn sanitize_stem(input: &str) -> String {
    let mut output = String::new();
    let mut separated = false;
    for character in input.chars() {
        if character.is_ascii_alphanumeric() {
            if separated && !output.is_empty() {
                output.push('-');
            }
            separated = false;
            output.push(character.to_ascii_lowercase());
        } else {
            separated = true;
        }
    }
    output
}

/// Lower-case one extension, keeping only ASCII letters and digits.
pub fn sanitize_extension(extension: &str) -> String {
    extension
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// Whether an extension inserts image Markdown.
pub fn is_image_extension(extension: &str) -> bool {
    let extension = sanitize_extension(extension);
    IMAGE_EXTENSIONS.contains(&extension.as_str())
}

/// Deterministic asset file name for one collision attempt.
///
/// Attempt zero is `YYYY-MM-DD-stem.ext`; each later attempt appends `-N`
/// before the extension. Empty extensions produce no dot.
pub fn asset_file_name(stem: &str, extension: Option<&str>, date: &str, attempt: usize) -> String {
    let suffix = if attempt == 0 {
        String::new()
    } else {
        format!("-{attempt}")
    };
    match extension {
        Some(extension) if !extension.is_empty() => {
            format!("{date}-{stem}{suffix}.{extension}")
        }
        _ => format!("{date}-{stem}{suffix}"),
    }
}

/// Image Markdown for images, a normal link for every other extension.
pub fn markdown_for_asset(alt: &str, path: &str, image: bool) -> String {
    let alt = escape_link_text(alt);
    let path = escape_link_destination(path);
    if image {
        format!("![{alt}]({path})")
    } else {
        format!("[{alt}]({path})")
    }
}

/// Path text for a project-relative target in the requested mode.
pub fn asset_path(mode: ImagePathMode, document: &str, target: &str) -> String {
    match mode {
        ImagePathMode::Relative => document_relative_path(document, target),
        ImagePathMode::Absolute => format!("/{}", target.trim_start_matches('/')),
    }
}

/// Calculate a document-relative path from one project-relative source.
///
/// Both arguments use `/` separators. The document's own segment is dropped
/// before the common prefix is removed, so a nested document reaches a sibling
/// collection with `..` instead of a project-root path.
pub fn document_relative_path(document: &str, target: &str) -> String {
    let mut document: Vec<&str> = document
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect();
    document.pop();
    let target: Vec<&str> = target
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect();
    let common = document
        .iter()
        .zip(target.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let mut segments = vec![".."; document.len() - common];
    segments.extend(target[common..].iter().copied());
    if segments.is_empty() {
        ".".to_owned()
    } else {
        segments.join("/")
    }
}

/// Project-relative `/`-separated text for an absolute path inside `root`.
pub fn project_relative_path(root: &Path, path: &Path) -> Result<String, AssetError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| AssetError::OutsideProject(path.to_path_buf()))?;
    let mut segments = Vec::new();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(AssetError::InvalidName(path.display().to_string()));
        };
        segments.push(
            segment
                .to_str()
                .ok_or_else(|| AssetError::InvalidName(path.display().to_string()))?,
        );
    }
    if segments.is_empty() {
        return Err(AssetError::InvalidName(path.display().to_string()));
    }
    Ok(segments.join("/"))
}

/// Escape a Content Finder link's visible text.
pub fn escape_link_text(text: &str) -> String {
    escape(text, |character| matches!(character, '[' | ']'))
}

/// Escape a Content Finder link destination.
///
/// Whitespace is percent-encoded so a bare destination cannot truncate at a
/// space, and parentheses and backslashes are backslash-escaped.
pub fn escape_link_destination(destination: &str) -> String {
    let mut escaped = String::with_capacity(destination.len());
    for character in destination.chars() {
        if character.is_whitespace() {
            let mut utf8 = [0u8; 4];
            for byte in character.encode_utf8(&mut utf8).as_bytes() {
                escaped.push('%');
                escaped.push(hex_digit(byte >> 4));
                escaped.push(hex_digit(byte & 0x0f));
            }
            continue;
        }
        if character == '\\' || matches!(character, '(' | ')') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn escape(text: &str, needs_escape: impl Fn(char) -> bool) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if character == '\\' || needs_escape(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Percent-encode one slug or entry id as a single URL path segment.
pub fn encode_url_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
    }
    encoded
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'A' + value - 10) as char,
    }
}

/// Content Finder link destination for one entry.
///
/// A non-empty URL pattern replaces every `{slug}` with the URL-encoded
/// frontmatter slug, falling back to the Astro entry id. Without a pattern the
/// already document-relative file path is used. Parentheses and backslashes in
/// the result are escaped for Markdown.
pub fn finder_link_target(
    pattern: Option<&str>,
    slug: Option<&str>,
    entry_id: &str,
    relative_source: &str,
) -> String {
    match pattern.map(str::trim).filter(|pattern| !pattern.is_empty()) {
        Some(pattern) => {
            let value = slug.filter(|slug| !slug.is_empty()).unwrap_or(entry_id);
            escape_link_destination(&pattern.replace("{slug}", &encode_url_segment(value)))
        }
        None => escape_link_destination(relative_source),
    }
}

fn assets_relative(root: &Path, configured: &Path) -> Result<PathBuf, AssetError> {
    if configured.is_absolute() {
        let relative = configured
            .strip_prefix(root)
            .map_err(|_| AssetError::OutsideProject(configured.to_path_buf()))?;
        return relative_components(relative);
    }
    relative_components(configured)
}

fn relative_components(path: &Path) -> Result<PathBuf, AssetError> {
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => relative.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AssetError::InvalidName(path.display().to_string()));
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(AssetError::InvalidName(path.display().to_string()));
    }
    Ok(relative)
}

fn lexical_join(root: &Path, relative: &Path) -> Result<PathBuf, AssetError> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(AssetError::InvalidName(relative.display().to_string()));
        };
        path.push(segment);
    }
    if !path.starts_with(root) {
        return Err(AssetError::OutsideProject(path));
    }
    Ok(path)
}

fn refuse_symlink_components(root: &Path, path: &Path) -> Result<(), AssetError> {
    let Ok(relative) = path.strip_prefix(root) else {
        return Err(AssetError::OutsideProject(path.to_path_buf()));
    };
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(AssetError::InvalidName(path.display().to_string()));
        };
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AssetError::Symlink(current));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(io_error(&current, error)),
        }
    }
    Ok(())
}

fn prepare_directory(location: &AssetLocation) -> Result<PathBuf, AssetError> {
    refuse_symlink_components(location.root(), location.directory())?;
    fs::create_dir_all(location.directory())
        .map_err(|error| io_error(location.directory(), error))?;
    let canonical = fs::canonicalize(location.directory())
        .map_err(|error| io_error(location.directory(), error))?;
    if !canonical.starts_with(location.root()) {
        return Err(AssetError::OutsideProject(canonical));
    }
    if !canonical.is_dir() {
        // `create_dir_all` already refuses an existing non-directory, so this
        // only guards a replacement between creation and canonicalisation.
        return Err(AssetError::NotADirectory(canonical));
    }
    Ok(canonical)
}

fn truncate_stem(stem: &str, extension: Option<&str>, date: &str) -> String {
    // Leave room for the date, both separators, the worst-case suffix and the
    // extension inside the common 255-byte file-name limit.
    let extension = extension.map(|extension| extension.len() + 1).unwrap_or(0);
    let fixed = date.len() + 1 + 6 + extension;
    let room = 255usize.saturating_sub(fixed);
    if stem.len() <= room {
        return stem.to_owned();
    }
    let mut truncated = String::new();
    for character in stem.chars() {
        if truncated.len() + character.len_utf8() > room {
            break;
        }
        truncated.push(character);
    }
    truncated.trim_end_matches('-').to_owned()
}

enum CopyAttempt {
    Collision,
    Failed(AssetError),
}

/// Write an asset to a destination that must not already exist.
///
/// `create_new` makes the name atomic, so a concurrent process cannot be
/// overwritten between the collision search and the write.
fn write_new_file(
    mut input: impl io::Read,
    destination: &Path,
    expected_len: u64,
) -> Result<(), CopyAttempt> {
    let mut output = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(CopyAttempt::Collision);
        }
        Err(error) => return Err(CopyAttempt::Failed(io_error(destination, error))),
    };
    let copied = io::copy(&mut input, &mut output).and_then(|length| {
        output.sync_all()?;
        Ok(length)
    });
    drop(output);
    let failure = match copied {
        Ok(length) if length == expected_len => return Ok(()),
        Ok(_) => AssetError::CopyMismatch(destination.to_path_buf()),
        Err(error) => io_error(destination, error),
    };
    let _ = fs::remove_file(destination);
    Err(CopyAttempt::Failed(failure))
}

fn io_error(path: &Path, error: io::Error) -> AssetError {
    AssetError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn location(root: &Path) -> AssetLocation {
        AssetLocation::resolve(root, None).unwrap()
    }

    fn request(source: PathBuf, document: &str, mode: ImagePathMode) -> AssetCopyRequest {
        AssetCopyRequest {
            source,
            date: "2026-11-03".into(),
            document: document.into(),
            path_mode: mode,
        }
    }

    #[test]
    fn stems_become_lower_kebab_and_unsafe_characters_disappear() {
        assert_eq!(sanitize_stem("My Photo (1).PNG"), "my-photo-1-png");
        assert_eq!(sanitize_stem("  Mix__Ed  --  Case  "), "mix-ed-case");
        assert_eq!(sanitize_stem("Über Café"), "ber-caf");
        assert_eq!(sanitize_stem("!!!"), "");
        assert_eq!(sanitize_extension("PNG"), "png");
        assert_eq!(sanitize_extension("t.a.r"), "tar");
    }

    #[test]
    fn names_are_dated_and_collision_suffixed_within_a_bound() {
        assert_eq!(
            asset_file_name("photo", Some("png"), "2026-11-03", 0),
            "2026-11-03-photo.png"
        );
        assert_eq!(
            asset_file_name("photo", Some("png"), "2026-11-03", 4),
            "2026-11-03-photo-4.png"
        );
        assert_eq!(
            asset_file_name("readme", None, "2026-11-03", 1),
            "2026-11-03-readme-1"
        );
    }

    #[test]
    fn nested_documents_reach_siblings_with_parent_segments() {
        assert_eq!(
            document_relative_path("src/posts/a.md", "src/assets/x.png"),
            "../assets/x.png"
        );
        assert_eq!(
            document_relative_path("src/posts/deep/a.md", "src/assets/x.png"),
            "../../assets/x.png"
        );
        assert_eq!(
            document_relative_path("a.md", "src/assets/x.png"),
            "src/assets/x.png"
        );
        assert_eq!(
            document_relative_path("src/posts/a.md", "src/posts/x.png"),
            "x.png"
        );
        assert_eq!(
            asset_path(
                ImagePathMode::Absolute,
                "src/posts/a.md",
                "src/assets/x.png"
            ),
            "/src/assets/x.png"
        );
    }

    #[test]
    fn url_patterns_replace_an_encoded_slug_and_escape_markdown() {
        assert_eq!(
            finder_link_target(Some("/blog/{slug}/"), Some("Hello World"), "id", "x.md"),
            "/blog/Hello%20World/"
        );
        assert_eq!(
            finder_link_target(Some("https://x.dev/{slug}?q=a(b)"), None, "my id/é", "x.md"),
            "https://x.dev/my%20id%2F%C3%A9?q=a\\(b\\)"
        );
        assert_eq!(
            finder_link_target(Some("/blog/{slug}"), Some(""), "fallback", "x.md"),
            "/blog/fallback"
        );
        assert_eq!(
            finder_link_target(None, Some("ignored"), "id", "../posts/a(b).md"),
            "../posts/a\\(b\\).md"
        );
        assert_eq!(
            finder_link_target(None, None, "id", "src/posts/my post.md"),
            "src/posts/my%20post.md"
        );
        assert_eq!(
            finder_link_target(Some("https://x.dev/a(b)"), None, "id", "x.md"),
            "https://x.dev/a\\(b\\)"
        );
        assert_eq!(
            finder_link_target(Some("   "), None, "id", "relative.md"),
            "relative.md"
        );
    }

    #[test]
    fn links_escape_text_brackets_and_destination_parentheses() {
        assert_eq!(escape_link_text("A [b] c\\d"), "A \\[b\\] c\\\\d");
        assert_eq!(
            escape_link_destination("../a(b)c\\.md"),
            "../a\\(b\\)c\\\\.md"
        );
        assert_eq!(
            markdown_for_asset("photo", "../a b.png", true),
            "![photo](../a%20b.png)"
        );
        assert_eq!(
            markdown_for_asset("notes", "../a(b).pdf", false),
            "[notes](../a\\(b\\).pdf)"
        );
    }

    #[test]
    fn only_the_fixed_raster_and_svg_set_is_an_image() {
        for image in [
            "png", "JPG", "jpeg", "gif", "webp", "avif", "bmp", "ico", "svg",
        ] {
            assert!(is_image_extension(image), "{image}");
        }
        for file in ["pdf", "md", "zip", "svgz", ""] {
            assert!(!is_image_extension(file), "{file}");
        }
    }

    #[test]
    fn copies_preserve_bytes_and_suffix_collisions() {
        let project = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let input = source.path().join("My Photo.PNG");
        let bytes = b"\x89PNG\r\n\x1a\nbinary\x00payload";
        fs::write(&input, bytes).unwrap();
        let location = location(project.path());
        let first = copy_asset(
            &location,
            &request(input.clone(), "src/posts/a.md", ImagePathMode::Relative),
        )
        .unwrap();
        assert_eq!(first.source(), "src/assets/2026-11-03-my-photo.png");
        assert_eq!(first.path(), "../assets/2026-11-03-my-photo.png");
        assert_eq!(
            first.markdown(),
            "![my-photo](../assets/2026-11-03-my-photo.png)"
        );
        assert!(first.is_image());
        let mut copied = Vec::new();
        fs::File::open(location.directory().join("2026-11-03-my-photo.png"))
            .unwrap()
            .read_to_end(&mut copied)
            .unwrap();
        assert_eq!(copied, bytes);

        let second = copy_asset(
            &location,
            &request(input, "src/posts/a.md", ImagePathMode::Absolute),
        )
        .unwrap();
        assert_eq!(second.source(), "src/assets/2026-11-03-my-photo-1.png");
        assert_eq!(second.path(), "/src/assets/2026-11-03-my-photo-1.png");
    }

    #[test]
    fn non_image_sources_insert_links_and_the_attempt_bound_is_enforced() {
        let project = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let input = source.path().join("Report (final).pdf");
        fs::write(&input, b"%PDF-1.7").unwrap();
        let location = location(project.path());
        fs::create_dir_all(location.directory()).unwrap();
        // Occupy both names the bounded copy may try.
        fs::write(
            location.directory().join("2026-11-03-report-final.pdf"),
            b"x",
        )
        .unwrap();
        let request = request(input.clone(), "notes/a.md", ImagePathMode::Relative);
        let error = copy_asset_bounded(&location, &request, 1).unwrap_err();
        assert_eq!(error, AssetError::NameAttemptsExhausted);

        let copied = copy_asset(&location, &request).unwrap();
        assert_eq!(copied.source(), "src/assets/2026-11-03-report-final-1.pdf");
        assert_eq!(copied.path(), "../src/assets/2026-11-03-report-final-1.pdf");
        assert!(!copied.is_image());
        assert_eq!(
            copied.markdown(),
            "[report-final](../src/assets/2026-11-03-report-final-1.pdf)"
        );
    }

    #[test]
    fn a_name_without_safe_characters_still_copies() {
        let project = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let input = source.path().join("фото.png");
        fs::write(&input, b"png").unwrap();
        let copied = copy_asset(
            &location(project.path()),
            &request(input, "a.md", ImagePathMode::Relative),
        )
        .unwrap();
        assert_eq!(copied.source(), "src/assets/2026-11-03-file.png");
        assert!(copied.is_image());
    }

    #[test]
    fn traversal_escapes_are_refused() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(
            AssetLocation::resolve(project.path(), Some("../outside")),
            Err(AssetError::InvalidName("../outside".into()))
        );
        assert_eq!(
            AssetLocation::resolve(project.path(), Some(outside.path().to_str().unwrap())),
            Err(AssetError::OutsideProject(outside.path().to_path_buf()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escapes_are_refused() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), project.path().join("linked")).unwrap();
        let escaped = AssetLocation::resolve(project.path(), Some("linked/assets")).unwrap();
        let source = tempfile::tempdir().unwrap();
        let input = source.path().join("a.png");
        fs::write(&input, b"png").unwrap();
        assert_eq!(
            copy_asset(
                &escaped,
                &request(input.clone(), "src/posts/a.md", ImagePathMode::Relative)
            ),
            Err(AssetError::Symlink(project.path().join("linked")))
        );

        let linked = source.path().join("linked.png");
        std::os::unix::fs::symlink(&input, &linked).unwrap();
        let location = location(project.path());
        assert_eq!(
            copy_asset(
                &location,
                &request(linked, "src/posts/a.md", ImagePathMode::Relative)
            ),
            Err(AssetError::Symlink(source.path().join("linked.png")))
        );
    }

    #[test]
    fn dates_and_missing_sources_are_refused() {
        let project = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let input = source.path().join("a.png");
        fs::write(&input, b"png").unwrap();
        let location = location(project.path());
        let mut invalid = request(input.clone(), "a.md", ImagePathMode::Relative);
        invalid.date = "2026-13-40".into();
        assert_eq!(
            copy_asset(&location, &invalid),
            Err(AssetError::InvalidDate("2026-13-40".into()))
        );
        let missing = request(
            source.path().join("missing.png"),
            "a.md",
            ImagePathMode::Relative,
        );
        assert_eq!(
            copy_asset(&location, &missing),
            Err(AssetError::Missing(source.path().join("missing.png")))
        );
    }
}
