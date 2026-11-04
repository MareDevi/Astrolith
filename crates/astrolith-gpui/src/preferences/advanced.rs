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
                "Global preferences".into(),
                format!("version {}", self.global),
            ),
            (
                "Project preferences".into(),
                format!("version {}", self.project),
            ),
            (
                "Project registry".into(),
                format!("version {}", self.registry),
            ),
        ]
    }
}

/// Entries Reset All removes. Nothing outside this list is touched.
pub(crate) fn reset_paths(folder: &Path) -> Vec<PathBuf> {
    vec![
        folder.join("global.json"),
        folder.join("projects.json"),
        folder.join("projects"),
        // The one-time version-1 import and its preserved original.
        folder.join("preferences.json"),
        folder.join("preferences.v1.migrated.json"),
    ]
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
            .unwrap_or_else(|| "Preferences folder unavailable".to_owned());
        let open = folder.clone();
        let open_button = Button::new("preferences-open-folder")
            .disabled(open.is_none())
            .label("Open preferences folder")
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(folder) = open.clone() {
                    this.open_folder(folder, cx);
                }
            }));

        div()
            .v_flex()
            .w_full()
            .gap_6()
            .child(self.section("Versions", None, rows, cx))
            .child(self.section(
                "Preferences folder",
                Some("Open the folder that holds the preference files."),
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
                "Reset",
                Some(
                    "Deletes only preferences and the project registry. Recovery snapshots, save \
                     failures and logs are kept. Astrolith restarts immediately.",
                ),
                vec![
                    Button::new("preferences-reset-all")
                        .danger()
                        .label("Reset all preferences…")
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
                    Ok(()) => "Opened the preferences folder.".to_owned(),
                    Err(error) => format!("Could not open the preferences folder: {error}"),
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
            self.notice = Some(SharedString::from(
                "The preferences folder is unavailable, so Reset All cannot run.",
            ));
            cx.notify();
            return;
        };
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let weak = weak.clone();
            alert
                .confirm()
                .title("Reset all preferences?")
                .description(format!(
                    "This deletes global and project preferences and the project registry in {}. \
                     Recovery snapshots, save-failure records and logs are kept. Astrolith \
                     restarts immediately and reopens with defaults.",
                    folder.display()
                ))
                .ok_text("Reset and restart")
                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                .cancel_text("Keep preferences")
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
    return Err("opening a file manager is unsupported on this platform".into());
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
            Err(format!("the file manager exited with {status}"))
        }
    }
}
