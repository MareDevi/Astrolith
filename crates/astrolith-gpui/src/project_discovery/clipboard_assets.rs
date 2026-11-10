use super::*;
use crate::editor::{assets::ClipboardAssets, image_upload};

impl ProjectWorkspace {
    /// Paste uses the original editor and selection across the asynchronous copy.
    pub(super) fn paste_assets(
        &mut self,
        assets: ClipboardAssets,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(active) = self.active.clone() else {
            return false;
        };
        let Some(session) = self.sessions.get(&active) else {
            return false;
        };
        let document_session = session.read(cx);
        if document_session.kind() == DocumentKind::Plain {
            return false;
        }
        let paste_location = document_session.paste_location(cx);
        let session = session.downgrade();
        let Some(document) = self.active_relative_source() else {
            self.status = tr!("browser.status.open_project_before_copy_assets");
            cx.notify();
            return true;
        };
        let collection = self.session_collections.get(&active).map(String::as_str);
        let location = match self.asset_location(collection) {
            Ok(location) => location,
            Err(error) => {
                self.status = tr!("browser.status.assets_dir_unavailable", error = error);
                cx.notify();
                return true;
            }
        };
        let root = location.root().to_path_buf();
        let mode = self.resolved_for(collection).image_path_mode;
        let date = asset_insertion::asset_date();
        let work = runtime::blocking(cx, move || assets.copy(&location, &date, &document, mode));
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let work = this.update_in(cx, |this, window, cx| {
                let (assets, failures) = match result {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        this.status = tr!("browser.status.assets_not_copied", error = error);
                        cx.notify();
                        return None;
                    }
                };
                if assets.is_empty() && failures.is_empty() {
                    return None;
                }
                let attached = session
                    .upgrade()
                    .filter(|session| this.sessions.get(&active) == Some(session));
                let inserted = !assets.is_empty()
                    && attached.is_some_and(|session| {
                        session.update(cx, |session, cx| {
                            session.insert_at_paste_location(
                                asset_insertion::drop_markdown(&assets),
                                paste_location,
                                window,
                                cx,
                            )
                        })
                    });
                this.status = if !assets.is_empty() && !inserted {
                    tr!("editor.clipboard.paste_moved")
                } else if let Some(failure) = asset_insertion::drop_failure_status(&failures) {
                    failure
                } else {
                    asset_insertion::drop_success_status(&assets)
                };
                let settings = this.global_preferences.image_upload.clone();
                let work =
                    (inserted && settings.enabled && assets.iter().any(|asset| asset.is_image()))
                        .then(|| {
                            runtime::blocking(cx, move || {
                                image_upload::upload_images(&settings, &root, &assets)
                            })
                        });
                if work.is_some() {
                    this.status = format!("{} {}", this.status, tr!("editor.clipboard.uploading"));
                }
                cx.notify();
                work
            });
            let Ok(Some(work)) = work else {
                return;
            };
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Ok((replacements, failed)) = result else {
                    this.status = tr!("editor.clipboard.upload_failed");
                    cx.notify();
                    return;
                };
                let updated = session
                    .upgrade()
                    .filter(|session| this.sessions.get(&active) == Some(session))
                    .map_or(0, |session| {
                        session.update(cx, |session, cx| {
                            session.replace_pasted_links(&replacements, window, cx)
                        })
                    });
                let mut messages = Vec::new();
                if updated > 0 {
                    messages.push(tr!("editor.clipboard.uploaded", count = updated));
                }
                if failed > 0 {
                    messages.push(tr!("editor.clipboard.upload_failed"));
                }
                if updated < replacements.len() {
                    messages.push(tr!("editor.clipboard.upload_link_changed"));
                }
                this.status = messages.join(" ");
                cx.notify();
            });
        });
        self.retain_command_task(task);
        true
    }
}
