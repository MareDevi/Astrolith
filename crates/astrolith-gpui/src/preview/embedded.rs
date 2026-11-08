//! Embedded WebView surface for Preview mode.
//!
//! The real adapter is compiled only with the `embedded-webview` feature. It
//! builds a Wry/WebKitGTK child of the GPUI window, keeps the entity alive for
//! the whole window session, and only changes URL or visibility afterwards.
//! Linux Wayland windows cannot host it; the caller falls back to the external
//! browser with the reported reason.

use url::Url;

use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
};
use gpui_kit::{
    App, Context, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Window, div,
    prelude::*,
};

/// Emitted when the embedded surface asks the shell for another action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EmbeddedPreviewEvent {
    /// Open the active entry in the configured external browser.
    OpenExternal,
}

/// Why the embedded surface cannot be shown.
#[derive(Clone, Debug)]
// One variant is only constructed by the other build configuration.
#[allow(dead_code)]
pub(crate) enum EmbeddedFailure {
    /// The native WebView could not be created.
    Create(String),
    /// This build was compiled without the embedded surface.
    FeatureDisabled,
}

impl EmbeddedFailure {
    /// The failure rendered in the current interface language.
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Create(error) => tr!("preview.embedded.create_failed", error = error.as_str()),
            Self::FeatureDisabled => tr!("preview.embedded.feature_disabled"),
        }
    }
}

/// A retained embedded WebView and its status.
pub(crate) struct EmbeddedPreview {
    url: Option<Url>,
    /// Why the surface could not be created, if it failed.
    failure: Option<EmbeddedFailure>,
    #[cfg(feature = "embedded-webview")]
    webview: Option<gpui_kit::Entity<gpui_webview::WebView>>,
    active: bool,
    focus: FocusHandle,
}

impl EventEmitter<EmbeddedPreviewEvent> for EmbeddedPreview {}

impl EmbeddedPreview {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            url: None,
            failure: None,
            #[cfg(feature = "embedded-webview")]
            webview: None,
            active: true,
            focus: cx.focus_handle(),
        }
    }

    /// Point the surface at `url`, building the native view on demand.
    ///
    /// `None` hides the view and keeps the loaded page. Repeating the same URL
    /// never rebuilds or reloads it.
    pub(crate) fn set_url(
        &mut self,
        url: Option<Url>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(url) = url else {
            self.hide(cx);
            self.url = None;
            cx.notify();
            return Ok(());
        };
        if self.url.as_ref() == Some(&url) {
            self.show(cx);
            return Ok(());
        }
        #[cfg(feature = "embedded-webview")]
        {
            if let Some(webview) = self.webview.clone() {
                let raw = url.to_string();
                webview.update(cx, |webview, _| webview.load_url(&raw));
                self.url = Some(url);
                self.failure = None;
                self.show(cx);
                cx.notify();
                return Ok(());
            }
            let builder = wry::WebViewBuilder::new().with_url(url.as_str());
            match gpui_webview::WebView::build(builder, window, cx) {
                Ok(webview) => {
                    tracing::info!(%url, "embedded preview WebView created");
                    self.webview = Some(cx.new(|_| webview));
                    self.url = Some(url);
                    self.failure = None;
                    self.show(cx);
                    cx.notify();
                    Ok(())
                }
                Err(error) => {
                    tracing::warn!(%error, "could not create the embedded WebView");
                    let failure = EmbeddedFailure::Create(error.to_string());
                    let message = failure.text();
                    self.failure = Some(failure);
                    self.url = None;
                    cx.notify();
                    Err(message)
                }
            }
        }
        #[cfg(not(feature = "embedded-webview"))]
        {
            let _ = window;
            let failure = EmbeddedFailure::FeatureDisabled;
            let message = failure.text();
            self.failure = Some(failure);
            self.url = None;
            cx.notify();
            Err(message)
        }
    }

    /// Show or hide the native view without freeing it.
    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.show(cx);
        } else {
            self.hide(cx);
        }
        cx.notify();
    }

    fn show(&mut self, cx: &mut Context<Self>) {
        let _ = cx;
        #[cfg(feature = "embedded-webview")]
        if self.active
            && let Some(webview) = self.webview.clone()
        {
            webview.update(cx, |webview, _| webview.show());
        }
    }

    fn hide(&mut self, cx: &mut Context<Self>) {
        #[cfg(feature = "embedded-webview")]
        if let Some(webview) = self.webview.clone() {
            webview.update(cx, |webview, _| webview.hide());
        }
        let _ = cx;
    }

    /// Reload the loaded page, if any.
    fn reload(&mut self, cx: &mut Context<Self>) {
        #[cfg(feature = "embedded-webview")]
        if let Some(webview) = self.webview.clone() {
            let _ = webview.update(cx, |webview, _| webview.raw().reload());
        }
        let _ = cx;
    }

    #[cfg(all(test, not(feature = "embedded-webview")))]
    pub(crate) fn has_webview(&self) -> bool {
        #[cfg(feature = "embedded-webview")]
        {
            self.webview.is_some()
        }
        #[cfg(not(feature = "embedded-webview"))]
        {
            false
        }
    }

    #[cfg(all(test, not(feature = "embedded-webview")))]
    pub(crate) fn failure(&self) -> Option<String> {
        self.failure.as_ref().map(EmbeddedFailure::text)
    }
}

impl Render for EmbeddedPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let url = self.url.as_ref().map(Url::to_string);
        let failure = self.failure.clone();
        let loading = url.is_none() && failure.is_none();
        let has_webview = {
            #[cfg(feature = "embedded-webview")]
            {
                self.webview.is_some()
            }
            #[cfg(not(feature = "embedded-webview"))]
            {
                false
            }
        };
        div()
            .id("embedded-preview")
            .v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.55))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(tr!("preview.embedded.title")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                url.clone()
                                    .unwrap_or_else(|| tr!("preview.embedded.no_page")),
                            ),
                    )
                    .child(
                        Button::new("embedded-preview-reload")
                            .xsmall()
                            .ghost()
                            .label(tr!("preview.actions.reload"))
                            .disabled(!has_webview)
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    )
                    .child(
                        Button::new("embedded-preview-open-external")
                            .xsmall()
                            .outline()
                            .label(tr!("preview.actions.open"))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(EmbeddedPreviewEvent::OpenExternal)
                            })),
                    ),
            )
            .child({
                #[cfg(feature = "embedded-webview")]
                if let Some(webview) = self.webview.clone() {
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(webview)
                        .into_any_element()
                } else {
                    self.status_body(loading, failure, cx)
                }
                #[cfg(not(feature = "embedded-webview"))]
                {
                    self.status_body(loading, failure, cx)
                }
            })
    }
}

impl EmbeddedPreview {
    fn status_body(
        &self,
        loading: bool,
        failure: Option<EmbeddedFailure>,
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let message = if let Some(failure) = failure {
            failure.text()
        } else if loading {
            tr!("preview.embedded.start_prompt")
        } else {
            tr!("preview.embedded.not_ready")
        };
        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .items_center()
            .justify_center()
            .p_6()
            .child(
                div()
                    .max_w(gpui_kit::px(360.))
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(message),
            )
            .into_any_element()
    }
}

impl Focusable for EmbeddedPreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
