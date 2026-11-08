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
                tr!("preferences.option.embedded")
            } else {
                tr!("preferences.option.embedded_unavailable")
            }
        } else {
            tr!("preferences.option.external_browser")
        };

        let presentation_weak = weak.clone();
        let presentation = Button::new("preferences-preview-presentation")
            .outline()
            .label(presentation_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let weak = presentation_weak.clone();
                let menu = menu.item(
                    PopupMenuItem::new(tr!("preferences.option.external_browser")).on_click(
                        move |_, _, cx| {
                            let _ = weak.update(cx, |sheet, cx| {
                                sheet.preview_presentation = PreviewPresentation::External;
                                cx.notify();
                            });
                        },
                    ),
                );
                if embedded_available {
                    let weak = presentation_weak.clone();
                    menu.item(
                        PopupMenuItem::new(tr!("preferences.option.embedded")).on_click(
                            move |_, _, cx| {
                                let _ = weak.update(cx, |sheet, cx| {
                                    sheet.preview_presentation = PreviewPresentation::Embedded;
                                    cx.notify();
                                });
                            },
                        ),
                    )
                } else {
                    menu.item(PopupMenuItem::new(tr!(
                        "preferences.option.embedded_unavailable"
                    )))
                }
            })
            .into_any_element();

        let enter_weak = weak.clone();
        let enter_label = if self.preview_open_on_enter {
            tr!("preferences.option.on")
        } else {
            tr!("preferences.option.off")
        };
        let open_on_enter = Button::new("preferences-preview-open-on-enter")
            .outline()
            .label(enter_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, value) in [
                    (tr!("preferences.option.off"), false),
                    (tr!("preferences.option.on"), true),
                ] {
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
                tr!("preferences.preview.presentation"),
                presentation,
                FieldExtras {
                    field: None,
                    hint: Some(tr!("preferences.preview.presentation_hint").into()),
                    trailing: None,
                },
                cx,
            ),
            self.field(
                "preferences-field-preview-browser-executable",
                tr!("preferences.preview.browser_executable"),
                self.control(FieldId::PreviewBrowserExecutable),
                FieldExtras {
                    field: Some(FieldId::PreviewBrowserExecutable),
                    hint: Some(tr!("preferences.preview.browser_executable_hint").into()),
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
                tr!("preferences.preview.browser_arguments"),
                self.control(FieldId::PreviewBrowserArguments),
                FieldExtras {
                    field: Some(FieldId::PreviewBrowserArguments),
                    hint: Some(tr!("preferences.preview.browser_arguments_hint").into()),
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
                tr!("preferences.preview.open_on_enter"),
                open_on_enter,
                FieldExtras {
                    field: None,
                    hint: Some(tr!("preferences.preview.open_on_enter_hint").into()),
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
                    .child(tr!(
                        "preferences.preview.embedded_unavailable_notice",
                        reason = reason
                    ))
                    .into_any_element(),
            );
        }
        self.section(
            tr!("preferences.preview.browser_section"),
            None,
            children,
            cx,
        )
    }
}
