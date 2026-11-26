//! Preview pane: presentation, preferred browser argv and Open on Enter.

use super::*;
use gpui_kit::component::switch::Switch;

/// Picker options for the preview presentation; the embedded host is unselectable when unavailable.
pub(super) fn presentation_options(
    selected: PreviewPresentation,
    embedded_available: bool,
) -> Vec<ui::PickerOption> {
    vec![
        ui::PickerOption::new(tr!("preferences.option.external_browser"))
            .selected(selected == PreviewPresentation::External),
        ui::PickerOption::new(if embedded_available {
            tr!("preferences.option.embedded")
        } else {
            tr!("preferences.option.embedded_unavailable")
        })
        .selected(selected == PreviewPresentation::Embedded)
        .disabled(!embedded_available),
    ]
}

impl PreferencesSheet {
    pub(super) fn preview_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
        let options = presentation_options(self.preview_presentation, embedded_available);
        let presentation = self.picker(
            "preferences-preview-presentation",
            presentation_label,
            options,
            None,
            cx,
            |sheet, index, _, cx| {
                sheet.preview_presentation = if index == 0 {
                    PreviewPresentation::External
                } else {
                    PreviewPresentation::Embedded
                };
                cx.notify();
            },
        );

        let open_on_enter = self.field(
            "preferences-field-preview-open-on-enter",
            tr!("preferences.preview.open_on_enter"),
            FieldBody::Control(None),
            FieldExtras {
                field: None,
                hint: Some(tr!("preferences.preview.open_on_enter_hint").into()),
                trailing: Some(
                    Switch::new("preferences-preview-open-on-enter")
                        .accessibility_label(tr!("preferences.preview.open_on_enter"))
                        .checked(self.preview_open_on_enter)
                        .on_change(cx.listener(|this, checked, _, cx| {
                            this.preview_open_on_enter = *checked;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
            },
            cx,
        );

        let mut children = vec![
            self.field(
                "preferences-field-preview-presentation",
                tr!("preferences.preview.presentation"),
                FieldBody::Control(Some(presentation)),
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
                FieldBody::Text(FieldId::PreviewBrowserExecutable),
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
                FieldBody::Text(FieldId::PreviewBrowserArguments),
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
            open_on_enter,
        ];
        if let Some(reason) = embedded_reason {
            children.push(
                ui::inline_notice(
                    "preferences-preview-embedded-unavailable",
                    NoticeTone::Warning,
                    tr!(
                        "preferences.preview.embedded_unavailable_notice",
                        reason = reason
                    ),
                )
                .into_any_element(),
            );
        }
        div()
            .v_flex()
            .w_full()
            .gap_6()
            .child(self.section(
                tr!("preferences.preview.browser_section"),
                None,
                children,
                cx,
            ))
            .into_any_element()
    }
}
