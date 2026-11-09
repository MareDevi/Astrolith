//! Advanced pane: versions, the preferences folder and Reset All.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use astrolith_core::preferences::{
    GLOBAL_PREFERENCES_VERSION, PROJECT_PREFERENCES_VERSION, PROJECT_REGISTRY_VERSION,
};

use gpui_kit::component::WindowExt;

use super::*;

/// Version information the Advanced pane displays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Versions {
    /// Astrolith application version.
    pub app: &'static str,
    /// `global.json` envelope version.
    pub global: u32,
    /// Per-project preference envelope version.
    pub project: u32,
    /// `projects.json` registry envelope version.
    pub registry: u32,
}

/// Versions of the running application and of every preference envelope.
pub(crate) fn versions() -> Versions {
    Versions {
        app: env!("CARGO_PKG_VERSION"),
        global: GLOBAL_PREFERENCES_VERSION,
        project: PROJECT_PREFERENCES_VERSION,
        registry: PROJECT_REGISTRY_VERSION,
    }
}

impl Versions {
    /// Rows in display order.
    pub(crate) fn lines(&self) -> Vec<(String, String)> {
        vec![
            ("Astrolith".into(), self.app.to_owned()),
            (
                tr!("preferences.advanced.global_preferences"),
                tr!("preferences.advanced.version", version = self.global),
            ),
            (
                tr!("preferences.advanced.project_preferences"),
                tr!("preferences.advanced.version", version = self.project),
            ),
            (
                tr!("preferences.advanced.project_registry"),
                tr!("preferences.advanced.version", version = self.registry),
            ),
        ]
    }
}

/// Entries Reset All removes. Nothing outside this list is touched.
///
/// Quarantined invalid files are removed too, so a reset leaves no
/// configuration artefact behind.
pub(crate) fn reset_paths(folder: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        folder.join("global.json"),
        folder.join("projects.json"),
        folder.join("projects"),
    ];
    if let Ok(entries) = fs::read_dir(folder) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let artifact = name.ends_with(".corrupt");
            if artifact {
                paths.push(entry.path());
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

/// Delete preferences and the registry, keeping recovery, save failures and logs.
///
/// The caller restarts the application afterwards; the shell suppresses every
/// pending and quit-time write so nothing recreates what this removed.
pub(crate) fn reset_preferences(folder: &Path) -> io::Result<()> {
    for path in reset_paths(folder) {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

impl PreferencesSheet {
    pub(super) fn advanced_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let folder = self
            .context
            .as_ref()
            .and_then(|context| context.preferences_folder.clone());
        let versions = versions();
        let rows = versions
            .lines()
            .into_iter()
            .enumerate()
            .map(|(index, (label, value))| {
                div()
                    .id(("preferences-version-row", index))
                    .h_flex()
                    .w_full()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(label),
                    )
                    .child(div().text_xs().child(value))
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let folder_label = folder
            .as_ref()
            .map(|folder| folder.display().to_string())
            .unwrap_or_else(|| tr!("preferences.advanced.folder_unavailable"));
        let open = folder.clone();
        let open_button = Button::new("preferences-open-folder")
            .disabled(open.is_none())
            .label(tr!("preferences.advanced.open_folder"))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(folder) = open.clone() {
                    this.open_folder(folder, cx);
                }
            }));

        div()
            .v_flex()
            .w_full()
            .gap_6()
            .child(self.section(tr!("preferences.advanced.versions"), None, rows, cx))
            .child(self.section(
                tr!("preferences.advanced.preferences_folder"),
                Some(tr!("preferences.advanced.preferences_folder_hint").into()),
                vec![
                    div()
                        .id("preferences-folder-path")
                        .test_support()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(folder_label)
                        .into_any_element(),
                    open_button.into_any_element(),
                ],
                cx,
            ))
            .child(self.section(
                tr!("preferences.advanced.reset_section"),
                Some(tr!("preferences.advanced.reset_section_hint").into()),
                vec![
                    Button::new("preferences-reset-all")
                        .danger()
                        .label(tr!("preferences.advanced.reset_all"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.confirm_reset_all(window, cx)
                        }))
                        .into_any_element(),
                ],
                cx,
            ))
            .into_any_element()
    }

    fn open_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        let opened = crate::runtime::spawn(cx, async move { open_directory(&folder).await });
        cx.spawn(async move |this, cx| {
            let result = opened.await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                this.notice = Some(SharedString::from(match result {
                    Ok(()) => tr!("preferences.advanced.folder_opened"),
                    Err(error) => tr!("preferences.advanced.folder_open_failed", error = error),
                }));
                cx.notify();
            });
        })
        .detach();
    }

    /// Reset All is destructive and names its exact scope in the confirmation.
    fn confirm_reset_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folder = self
            .context
            .as_ref()
            .and_then(|context| context.preferences_folder.clone());
        let Some(folder) = folder else {
            self.notice = Some(SharedString::from(tr!(
                "preferences.advanced.folder_unavailable_notice"
            )));
            cx.notify();
            return;
        };
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let weak = weak.clone();
            alert
                .confirm()
                .title(tr!("preferences.advanced.reset_title"))
                .description(tr!(
                    "preferences.advanced.reset_body",
                    folder = folder.display()
                ))
                .ok_text(tr!("preferences.advanced.reset_confirm"))
                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                .cancel_text(tr!("preferences.advanced.reset_cancel"))
                .show_cancel(true)
                .on_ok(move |_, window, cx| {
                    let _ = weak.update(cx, |sheet, cx| {
                        cx.emit(PreferencesEvent::ResetAll);
                        sheet.cancel(cx);
                    });
                    window.close_all_dialogs(cx);
                    true
                })
        });
    }
}

async fn open_directory(directory: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = tokio::process::Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = tokio::process::Command::new("xdg-open");
    #[cfg(not(any(unix, windows)))]
    return Err(tr!("preferences.advanced.file_manager_unsupported"));
    #[allow(unreachable_code)]
    {
        let status = command
            .arg(directory)
            .status()
            .await
            .map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(tr!(
                "preferences.advanced.file_manager_failed",
                status = status
            ))
        }
    }
}
