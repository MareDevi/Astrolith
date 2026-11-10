//! Asset insertion shared by editor drops and the frontmatter image control.
//!
//! Copying itself lives in `astrolith_workspace::assets`; this module only
//! batches one drop, formats the resulting Markdown and keeps user-facing
//! failures separate from copied files.

use std::path::PathBuf;

use astrolith_workspace::assets::{
    AssetCopyRequest, AssetLocation, CopiedAsset, copy_asset, write_clipboard_image,
};
use gpui_kit::{ClipboardEntry, ClipboardItem, Image};

pub(crate) enum ClipboardAssets {
    Files(Vec<PathBuf>),
    Images(Vec<Image>),
    #[cfg(target_os = "linux")]
    LinuxFiles,
}

impl ClipboardAssets {
    pub(crate) fn from_item(item: Option<&ClipboardItem>) -> Option<Self> {
        #[cfg(target_os = "linux")]
        if item.is_none() {
            return Some(Self::LinuxFiles);
        }
        let item = item?;
        let paths: Vec<_> = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()),
                _ => None,
            })
            .flatten()
            .cloned()
            .collect();
        if !paths.is_empty() {
            return Some(Self::Files(paths));
        }
        let images: Vec<_> = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                ClipboardEntry::Image(image) => Some(image.clone()),
                _ => None,
            })
            .collect();
        if !images.is_empty() {
            return Some(Self::Images(images));
        }
        // Some file managers also expose their file URI list as clipboard text.
        let text = item.text()?;
        let text = text
            .strip_prefix("copy\n")
            .or_else(|| text.strip_prefix("cut\n"))
            .unwrap_or(&text);
        let paths: Option<Vec<_>> = text
            .lines()
            .map(|line| url::Url::parse(line).ok()?.to_file_path().ok())
            .collect();
        paths.filter(|paths| !paths.is_empty()).map(Self::Files)
    }

    pub(crate) fn copy(
        self,
        location: &AssetLocation,
        date: &str,
        document: &str,
        mode: astrolith_core::preferences::ImagePathMode,
    ) -> DropOutcome {
        match self {
            Self::Files(paths) => copy_dropped(location, &paths, date, document, mode),
            Self::Images(images) => {
                let mut assets = Vec::new();
                let mut failures = Vec::new();
                for image in images {
                    let extension = image.format.extension();
                    if !image.bytes.is_empty()
                        && let Ok(asset) = write_clipboard_image(
                            location,
                            extension,
                            &image.bytes,
                            date,
                            document,
                            mode,
                        )
                    {
                        assets.push(asset);
                    } else {
                        failures.push(format!("clipboard-image.{extension}"));
                    }
                }
                (assets, failures)
            }
            #[cfg(target_os = "linux")]
            Self::LinuxFiles => {
                let paths = arboard::Clipboard::new()
                    .and_then(|mut clipboard| clipboard.get().file_list())
                    .unwrap_or_default();
                copy_dropped(location, &paths, date, document, mode)
            }
        }
    }
}

/// One copied asset or the failure of one source file.
pub(crate) type DropOutcome = (Vec<CopiedAsset>, Vec<String>);

/// Copy every dropped source through the asset service.
///
/// Successful files are returned in drop order so their Markdown follows the
/// pointer's order. A failure never substitutes the original host path.
pub(crate) fn copy_dropped(
    location: &AssetLocation,
    sources: &[std::path::PathBuf],
    date: &str,
    document: &str,
    path_mode: astrolith_core::preferences::ImagePathMode,
) -> DropOutcome {
    let mut copied = Vec::new();
    let mut failures = Vec::new();
    for source in sources {
        let request = AssetCopyRequest {
            source: source.clone(),
            date: date.to_owned(),
            document: document.to_owned(),
            path_mode,
        };
        match copy_asset(location, &request) {
            Ok(asset) => copied.push(asset),
            // The status bar names the file only; never the host path.
            Err(_) => failures.push(
                source
                    .file_name()
                    .unwrap_or(source.as_os_str())
                    .to_string_lossy()
                    .into_owned(),
            ),
        }
    }
    (copied, failures)
}

/// Markdown for one drop, one asset per line.
pub(crate) fn drop_markdown(assets: &[CopiedAsset]) -> String {
    assets
        .iter()
        .map(|asset| asset.markdown())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Local `YYYY-MM-DD` prefix for copied asset names.
pub(crate) fn asset_date() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Status text for a completed drop; `None` means every file was copied.
pub(crate) fn drop_failure_status(failures: &[String]) -> Option<String> {
    if failures.is_empty() {
        return None;
    }
    let names = failures.join("; ");
    Some(if failures.len() == 1 {
        tr!("editor.drop.failure_one", names = names)
    } else {
        tr!(
            "editor.drop.failure_many",
            count = failures.len(),
            names = names
        )
    })
}

/// Status text for a fully copied drop, naming the copied files.
pub(crate) fn drop_success_status(assets: &[CopiedAsset]) -> String {
    let images = assets.iter().filter(|asset| asset.is_image()).count();
    let names = assets
        .iter()
        .map(|asset| asset.source())
        .collect::<Vec<_>>()
        .join(", ");
    match (assets.len(), images) {
        (1, 0) => tr!("editor.drop.success_one_file", names = names),
        (1, _) => tr!("editor.drop.success_one_image", names = names),
        (count, 0) => tr!("editor.drop.success_files", count = count, names = names),
        (count, 1) => tr!(
            "editor.drop.success_one_image_of",
            count = count,
            names = names
        ),
        (count, images) => tr!(
            "editor.drop.success_with_images",
            count = count,
            images = images,
            names = names
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolith_core::preferences::ImagePathMode;
    use std::fs;

    #[test]
    fn drops_insert_image_markdown_for_raster_and_links_for_pdf() {
        let project = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let image = source.path().join("My Image.png");
        let pdf = source.path().join("Paper.pdf");
        fs::write(&image, b"png").unwrap();
        fs::write(&pdf, b"%PDF").unwrap();
        let location = AssetLocation::resolve(project.path(), None).unwrap();
        let (assets, failures) = copy_dropped(
            &location,
            &[image, pdf],
            "2026-11-03",
            "src/posts/a.md",
            ImagePathMode::Relative,
        );
        assert!(failures.is_empty());
        assert_eq!(assets.len(), 2);
        assert!(assets[0].is_image());
        assert!(!assets[1].is_image());
        assert_eq!(
            drop_markdown(&assets),
            "![my-image](../assets/2026-11-03-my-image.png)\n\
             [paper](../assets/2026-11-03-paper.pdf)"
        );
        assert_eq!(
            drop_success_status(&assets),
            "Copied 2 assets (1 image): src/assets/2026-11-03-my-image.png, \
             src/assets/2026-11-03-paper.pdf"
        );
    }

    #[test]
    fn a_failed_source_is_reported_without_a_raw_path() {
        let project = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let missing = source.path().join("missing.png");
        fs::write(source.path().join("ok.png"), b"png").unwrap();
        let location = AssetLocation::resolve(project.path(), None).unwrap();
        let (assets, failures) = copy_dropped(
            &location,
            &[missing, source.path().join("ok.png")],
            "2026-11-03",
            "a.md",
            ImagePathMode::Relative,
        );
        assert_eq!(assets.len(), 1);
        assert_eq!(failures, vec!["missing.png".to_owned()]);
        assert_eq!(
            drop_failure_status(&failures).unwrap(),
            "1 asset was not copied: missing.png"
        );
        assert_eq!(drop_failure_status(&[]), None);
        assert_eq!(
            drop_failure_status(&["a.png".to_owned(), "b.pdf".to_owned()]).unwrap(),
            "2 assets were not copied: a.png; b.pdf"
        );
    }
}
