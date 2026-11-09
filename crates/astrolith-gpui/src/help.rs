//! Bundled in-app help: the user guide, the shortcut reference and local paths.
//!
//! The Help menu must open real bundled documentation rather than a dead
//! external URL. The user guide is compiled from `docs/user/guide.md`; the
//! shortcut reference is generated from the same command registry that menus,
//! keys and the palette use, so it cannot drift from the application. The
//! local-data surface lists resolved directories only; it never reads file
//! contents.

use astrolith_core::preferences::CopyeditPreferences;
use gpui_kit::component::{ActiveTheme, StyledExt, kbd::Kbd, scroll::ScrollableElement};
use gpui_kit::{AnyElement, App, IntoElement, SharedString, div, prelude::*, px};

use crate::commands::{COMMANDS, CommandId, command_group, command_group_names};
use crate::settings::{recovery_dir, save_failure_dir, settings_path};

/// The user guide bundled into the application.
pub(crate) const USER_GUIDE: &str = include_str!("../../../docs/user/guide.md");

/// One help surface reachable from the existing Help menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelpSurface {
    /// The bundled user guide.
    UserGuide,
    /// Shortcuts generated from the live command registry.
    KeyboardShortcuts,
    /// Resolved local directories, without file contents.
    LocalData,
}

impl HelpSurface {
    /// Dialog title for this surface.
    ///
    /// The title matches the command that opens the surface, so the menu and
    /// the dialog never name the same thing differently.
    pub(crate) fn title(self) -> String {
        match self {
            Self::UserGuide => tr!("commands.label.user_guide"),
            Self::KeyboardShortcuts => tr!("commands.label.keyboard_shortcuts"),
            Self::LocalData => tr!("commands.label.privacy_and_local_data"),
        }
    }
}

/// One rendered shortcut row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShortcutRow {
    /// Command label under the current Copyedit state.
    pub label: String,
    /// Linux-formatted key sequence.
    pub keys: String,
}

/// One heading of the guide after markdown-lite parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GuideBlock {
    /// The document title.
    Title(String),
    /// A section heading.
    Heading(String),
    /// A joined paragraph.
    Paragraph(String),
    /// One bullet item.
    Bullet(String),
}

/// Parse the guide into plain-text blocks for the in-app surface.
///
/// Consecutive source lines become one paragraph so repository line wrapping
/// does not turn into hard breaks inside the dialog.
pub(crate) fn guide_blocks(source: &str) -> Vec<GuideBlock> {
    let mut blocks = Vec::new();
    let mut paragraph = String::new();
    for line in source.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if !paragraph.is_empty() {
                blocks.push(GuideBlock::Paragraph(std::mem::take(&mut paragraph)));
            }
            continue;
        }
        if let Some(title) = line.strip_prefix("# ") {
            if !paragraph.is_empty() {
                blocks.push(GuideBlock::Paragraph(std::mem::take(&mut paragraph)));
            }
            blocks.push(GuideBlock::Title(title.trim().to_owned()));
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ") {
            if !paragraph.is_empty() {
                blocks.push(GuideBlock::Paragraph(std::mem::take(&mut paragraph)));
            }
            blocks.push(GuideBlock::Heading(heading.trim().to_owned()));
            continue;
        }
        if let Some(item) = line.strip_prefix("- ") {
            if !paragraph.is_empty() {
                blocks.push(GuideBlock::Paragraph(std::mem::take(&mut paragraph)));
            }
            blocks.push(GuideBlock::Bullet(item.trim().to_owned()));
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(line.trim_start());
    }
    if !paragraph.is_empty() {
        blocks.push(GuideBlock::Paragraph(paragraph));
    }
    blocks
}

/// The Linux-formatted key sequence for one command, if it is bound.
pub(crate) fn formatted_shortcut(command: CommandId) -> Option<String> {
    command
        .shortcut()
        .and_then(|key| gpui_kit::Keystroke::parse(key).ok())
        .map(|key| Kbd::format(&key))
}

/// Shortcut rows grouped like the menus, in menu order.
pub(crate) fn shortcut_groups(copyedit: &CopyeditPreferences) -> Vec<(String, Vec<ShortcutRow>)> {
    let mut groups: Vec<(String, Vec<ShortcutRow>)> = command_group_names()
        .into_iter()
        .map(|title| (title, Vec::new()))
        .collect();
    for command in COMMANDS {
        let Some(keys) = formatted_shortcut(*command) else {
            continue;
        };
        let group = command_group(*command);
        if let Some((_, rows)) = groups.iter_mut().find(|(title, _)| *title == group) {
            rows.push(ShortcutRow {
                label: command.label_with(copyedit),
                keys,
            });
        }
    }
    groups.retain(|(_, rows)| !rows.is_empty());
    groups
}

/// One resolved local directory shown by the privacy surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalPathRow {
    /// Translation key describing what is stored there.
    pub label_key: &'static str,
    /// Resolved path, or the reason it is unavailable.
    pub path: String,
}

fn display_path(path: Result<std::path::PathBuf, std::io::Error>) -> String {
    match path {
        Ok(path) => path.display().to_string(),
        Err(error) => tr!("commands.help.local_data.unavailable", reason = error),
    }
}

/// Resolve every local data directory without reading any file.
pub(crate) fn local_paths() -> Vec<LocalPathRow> {
    let preferences = settings_path()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .map_or_else(
            || tr!("commands.help.local_data.no_preferences_dir"),
            |path| path.display().to_string(),
        );
    vec![
        LocalPathRow {
            label_key: "commands.help.local_data.preferences",
            path: preferences,
        },
        LocalPathRow {
            label_key: "commands.help.local_data.recovery",
            path: display_path(recovery_dir()),
        },
        LocalPathRow {
            label_key: "commands.help.local_data.save_failures",
            path: display_path(save_failure_dir()),
        },
        LocalPathRow {
            label_key: "commands.help.local_data.logs",
            path: display_path(astrolith_observability::default_log_dir()),
        },
    ]
}

/// Render the bundled guide as plain text inside a scrollable body.
pub(crate) fn guide_body(cx: &App) -> AnyElement {
    div()
        .id("help-user-guide")
        .v_flex()
        .w_full()
        .gap_1()
        .max_h(px(520.))
        .overflow_y_scrollbar()
        .children(guide_blocks(USER_GUIDE).into_iter().map(|block| {
            match block {
                GuideBlock::Title(text) => div()
                    .font_family(crate::theme::display_font(cx))
                    .text_lg()
                    .font_semibold()
                    .pb_1()
                    .child(SharedString::from(text))
                    .into_any_element(),
                GuideBlock::Heading(text) => div()
                    .font_family(crate::theme::display_font(cx))
                    .text_sm()
                    .font_semibold()
                    .pt_2()
                    .child(SharedString::from(text))
                    .into_any_element(),
                GuideBlock::Paragraph(text) => div()
                    .text_xs()
                    .w_full()
                    .child(SharedString::from(text))
                    .into_any_element(),
                GuideBlock::Bullet(text) => div()
                    .text_xs()
                    .w_full()
                    .pl_3()
                    .child(SharedString::from(format!("• {text}")))
                    .into_any_element(),
            }
        }))
        .into_any_element()
}

/// Render the generated shortcut reference.
pub(crate) fn shortcuts_body(groups: &[(String, Vec<ShortcutRow>)], cx: &App) -> AnyElement {
    div()
        .id("help-keyboard-shortcuts")
        .v_flex()
        .w_full()
        .gap_1()
        .max_h(px(520.))
        .overflow_y_scrollbar()
        .children(groups.iter().flat_map(|(title, rows)| {
            let mut elements = vec![
                div()
                    .font_family(crate::theme::display_font(cx))
                    .text_sm()
                    .font_semibold()
                    .pt_2()
                    .child(SharedString::from(title.clone()))
                    .into_any_element(),
            ];
            elements.extend(rows.iter().map(|row| {
                div()
                    .h_flex()
                    .w_full()
                    .justify_between()
                    .gap_4()
                    .text_xs()
                    .child(SharedString::from(row.label.clone()))
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(row.keys.clone())),
                    )
                    .into_any_element()
            }));
            elements
        }))
        .into_any_element()
}

/// Render the local-data inventory: paths only, never contents.
pub(crate) fn local_data_body(rows: &[LocalPathRow], cx: &App) -> AnyElement {
    div()
        .id("help-local-data")
        .v_flex()
        .w_full()
        .gap_2()
        .max_h(px(520.))
        .overflow_y_scrollbar()
        .child(
            div()
                .text_xs()
                .w_full()
                .child(tr!("commands.help.local_data.privacy_notice")),
        )
        .children(rows.iter().map(|row| {
            div()
                .v_flex()
                .w_full()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(tr!(row.label_key))),
                )
                .child(div().text_sm().child(SharedString::from(row.path.clone())))
                .into_any_element()
        }))
        .child(
            div()
                .text_xs()
                .w_full()
                .pt_2()
                .child(tr!("commands.help.local_data.delete_warning")),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn guide_blocks_join_paragraphs_and_keep_headings() {
        let blocks = guide_blocks("# Title\n\n## Section\nOne line\nsecond line\n\n- item\n");
        assert_eq!(
            blocks,
            vec![
                GuideBlock::Title("Title".into()),
                GuideBlock::Heading("Section".into()),
                GuideBlock::Paragraph("One line second line".into()),
                GuideBlock::Bullet("item".into()),
            ]
        );
    }

    #[test]
    fn bundled_guide_is_repository_documentation() {
        assert!(USER_GUIDE.contains("# Astrolith user guide"));
        assert!(USER_GUIDE.contains("## 14. Linux limits and known gaps"));
        assert!(!USER_GUIDE.trim().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shortcut_reference_covers_every_registered_shortcut() {
        let reference = include_str!("../../../docs/user/keyboard-shortcuts.md");
        for command in COMMANDS {
            let Some(keys) = formatted_shortcut(*command) else {
                continue;
            };
            assert!(
                reference.contains(&keys),
                "the shortcut reference is missing `{keys}` for {}",
                command.label()
            );
        }
    }

    #[test]
    fn shortcut_groups_follow_the_menu_order_and_skip_unbound_commands() {
        let groups = shortcut_groups(&CopyeditPreferences::default());
        let titles: Vec<_> = groups.iter().map(|(title, _)| title.as_str()).collect();
        assert_eq!(
            titles,
            ["File", "View", "Format", "Writer", "Preview", "Application"]
        );
        let format: Vec<_> = groups[2].1.iter().map(|row| row.keys.as_str()).collect();
        assert!(format.contains(&formatted_shortcut(CommandId::Bold).unwrap().as_str()));
        assert!(
            format.contains(
                &formatted_shortcut(CommandId::ToggleComment)
                    .unwrap()
                    .as_str()
            )
        );
        assert!(
            !groups
                .iter()
                .flat_map(|(_, rows)| rows)
                .any(|row| row.label == "About Astrolith")
        );
    }

    #[test]
    fn local_paths_are_directories_without_document_content() {
        let rows = local_paths();
        assert_eq!(rows.len(), 4);
        for row in &rows {
            assert!(!row.path.is_empty());
            assert!(!row.path.contains('\n'));
        }
        for (row, leaf) in rows
            .iter()
            .zip(["preferences", "recovery", "save-failures", "logs"])
        {
            if !row.path.starts_with("Unavailable") {
                assert!(
                    row.path.ends_with(&format!("astrolith/{leaf}")),
                    "{} should end with astrolith/{leaf}, got {}",
                    row.label_key,
                    row.path
                );
            }
        }
    }

    /// Every relative Markdown link in the documents P15 owns must resolve.
    #[test]
    fn user_and_derived_documentation_links_resolve() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let documents = [
            "README.md",
            "docs/user/README.md",
            "docs/user/guide.md",
            "docs/user/keyboard-shortcuts.md",
            "docs/user/privacy.md",
        ];
        let mut checked = 0;
        for document in documents {
            let path = root.join(document);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
            let base = path.parent().unwrap_or(&root);
            for target in markdown_link_targets(&text) {
                checked += 1;
                let target = target.split('#').next().unwrap_or("");
                if target.is_empty()
                    || target.contains("://")
                    || target.starts_with("mailto:")
                    || !target.ends_with(".md")
                {
                    continue;
                }
                let resolved = if target.starts_with('/') {
                    PathBuf::from(target)
                } else {
                    base.join(target)
                };
                assert!(
                    resolved.exists(),
                    "{} links to missing `{target}`",
                    path.display()
                );
            }
        }
        assert!(checked > 10, "expected the docs to contain links");
    }

    fn markdown_link_targets(text: &str) -> Vec<&str> {
        let mut targets = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find("](") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find(')') else {
                break;
            };
            let target = rest[..end].trim();
            if !target.starts_with('`') {
                targets.push(target);
            }
            rest = &rest[end + 1..];
        }
        targets
    }
}
