//! Preview pane: presentation, preferred browser argv and Open on Enter.

use super::*;

impl PreferencesSheet {
    pub(super) fn preview_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let embedded_reason = self
            .context
            .as_ref()
            .and_then(|context| context.embedded_unavailable_reason.clone());
        let embedded_available = embedded_reason.is_none();
        let presentation_label = if self.preview_presentation == PreviewPresentation::Embedded {
            if embedded_available {
                "Embedded WebView"
            } else {
                "Embedded WebView (unavailable)"
            }
        } else {
            "External browser"
        };

        let presentation_weak = weak.clone();
        let presentation = Button::new("preferences-preview-presentation")
            .outline()
            .label(presentation_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let weak = presentation_weak.clone();
                let menu = menu.item(PopupMenuItem::new("External browser").on_click(
                    move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.preview_presentation = PreviewPresentation::External;
                            cx.notify();
                        });
                    },
                ));
                if embedded_available {
                    let weak = presentation_weak.clone();
                    menu.item(
                        PopupMenuItem::new("Embedded WebView").on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |sheet, cx| {
                                sheet.preview_presentation = PreviewPresentation::Embedded;
                                cx.notify();
                            });
                        }),
                    )
                } else {
                    menu.item(PopupMenuItem::new("Embedded WebView (unavailable)"))
                }
            })
            .into_any_element();

        let enter_weak = weak.clone();
        let enter_label = if self.preview_open_on_enter {
            "On"
        } else {
            "Off"
        };
        let open_on_enter = Button::new("preferences-preview-open-on-enter")
            .outline()
            .label(enter_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, value) in [("Off", false), ("On", true)] {
                    let weak = enter_weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |sheet, cx| {
                            sheet.preview_open_on_enter = value;
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element();

        let mut children = vec![
            self.field(
                "preferences-field-preview-presentation",
                "Presentation",
                presentation,
                FieldExtras {
                    field: None,
                    hint: Some(
                        "Embedded WebView is available only on supported hosts; Wayland windows use the external browser.",
                    ),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-preview-browser-executable",
                "Preferred browser executable",
                self.control(FieldId::PreviewBrowserExecutable),
                FieldExtras {
                    field: Some(FieldId::PreviewBrowserExecutable),
                    hint: Some(
                        "Empty uses the system default browser. The executable is started directly, without a shell.",
                    ),
                    trailing: Some(self.reset_button(
                        FieldId::PreviewBrowserExecutable,
                        "preferences-reset-preview-browser-executable",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-preview-browser-arguments",
                "Browser arguments",
                self.control(FieldId::PreviewBrowserArguments),
                FieldExtras {
                    field: Some(FieldId::PreviewBrowserArguments),
                    hint: Some(
                        "One argument per line. {url} is replaced with the preview URL; without it the URL is appended.",
                    ),
                    trailing: Some(self.reset_button(
                        FieldId::PreviewBrowserArguments,
                        "preferences-reset-preview-browser-arguments",
                        cx,
                    )),
                },
                cx,
            ),
            self.field(
                "preferences-field-preview-open-on-enter",
                "Open on Enter",
                open_on_enter,
                FieldExtras {
                    field: None,
                    hint: Some(
                        "Open the active entry once when you explicitly enter Preview. A failed launch offers the system browser.",
                    ),
                    trailing: None,
                },
                cx,
            ),
        ];
        if let Some(reason) = embedded_reason {
            children.push(
                div()
                    .id("preferences-preview-embedded-unavailable")
                    .test_support()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(format!("Embedded WebView unavailable: {reason}"))
                    .into_any_element(),
            );
        }
        self.section("Browser", None, children, cx)
    }
}
