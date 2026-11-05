//! Application-owned typography for the writing desk.
//!
//! The mock sets the interface and the document in Inter. One family is
//! resolved per process from installed fonts, because GPUI silently falls back
//! to the system UI face when a requested family is absent. Without Inter the
//! platform UI family is the closest measured fallback; CJK coverage comes from
//! GPUI's per-script fallback.

use std::sync::OnceLock;

use astrolith_core::preferences::GlobalPreferencesV2;
use gpui_kit::{App, SharedString, component::ActiveTheme};

/// Prose column width for a surface created before preferences are resolved.
///
/// This is the only fallback: everything else derives the width from the
/// resolved [`GlobalPreferencesV2::page_width_percent`].
pub(crate) fn default_prose_page_width_percent() -> f32 {
    GlobalPreferencesV2::default().page_width_percent
}

/// Horizontal inset inside the prose column.
pub(crate) const PROSE_INSET: f32 = 32.;

/// Vertical breathing room above the first line and below the last.
pub(crate) const PROSE_TOP: f32 = 96.;

/// The mock's interface and prose face, under the names font managers use.
const UI_ALTERNATES: &[&str] = &["Inter Variable", "Inter", "Inter Display"];

/// The virtual family GPUI resolves on every platform.
const SYSTEM_UI_FONT: &str = ".SystemUIFont";

/// The mock's measured sans face, or the platform UI face when Inter is absent.
pub(crate) fn preferred_ui_font(cx: &App) -> SharedString {
    static RESOLVED: OnceLock<SharedString> = OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            let candidate = first_installed(UI_ALTERNATES, installed_font_names(cx));
            if candidate == SYSTEM_UI_FONT {
                system_ui_font(cx)
            } else {
                candidate
            }
        })
        .clone()
}

/// The default document face: the mock's sans face.
pub(crate) fn prose_font(cx: &App) -> SharedString {
    preferred_ui_font(cx)
}

/// The configured document face, falling back to the mock's sans face when the
/// preference is empty or the installed family disappeared.
pub(crate) fn configured_prose_font(cx: &App, family: &str) -> SharedString {
    let family = family.trim();
    if family.is_empty() {
        return prose_font(cx);
    }
    if font_family_available(cx, family) {
        return family.to_owned().into();
    }
    tracing::warn!(
        family,
        "document font is not installed; using the default sans face"
    );
    prose_font(cx)
}

/// The platform's resolved UI family, captured before a preference overrides it.
pub(crate) fn system_ui_font(cx: &App) -> SharedString {
    static RESOLVED: OnceLock<SharedString> = OnceLock::new();
    RESOLVED
        .get_or_init(|| cx.theme().font_family.clone())
        .clone()
}

/// The face for interface headings.
///
/// An explicit interface family is used everywhere, including these headings,
/// so the setting means what it says.
pub(crate) fn display_font(cx: &App) -> SharedString {
    let family = cx.theme().font_family.clone();
    if family == system_ui_font(cx) {
        prose_font(cx)
    } else {
        family
    }
}

/// Whether `family` names an installed family. An empty value means the
/// platform UI font; when the platform reports no fonts at all, accept the
/// value and let GPUI's fallback resolve it.
pub(crate) fn font_family_available(cx: &App, family: &str) -> bool {
    let family = family.trim();
    if family.is_empty() {
        return true;
    }
    let installed = installed_font_names(cx);
    installed.is_empty() || family_available_in(installed, family)
}

/// Exact, case-insensitive family match against an installed list.
fn family_available_in(installed: &[String], family: &str) -> bool {
    installed
        .iter()
        .any(|name| name.eq_ignore_ascii_case(family))
}

/// The families installed on this machine, listed once per process.
///
/// Enumerating fonts can take a hundred milliseconds on macOS and the system
/// font set does not change while the application runs.
fn installed_font_names(cx: &App) -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| cx.text_system().all_font_names())
}

/// The first installed candidate, or the system UI face when none is present.
fn first_installed(alternates: &[&str], installed: &[String]) -> SharedString {
    alternates
        .iter()
        .find(|candidate| installed.iter().any(|name| name == *candidate))
        .copied()
        .unwrap_or(SYSTEM_UI_FONT)
        .to_string()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn picks_the_first_installed_ui_face() {
        let installed = names(&["DejaVu Sans", "Inter", ".SystemUIFont"]);
        assert_eq!(
            first_installed(&["Inter Variable", "Inter"], &installed),
            SharedString::from("Inter")
        );
        assert_eq!(
            first_installed(&["Inter Variable", "Noto Sans"], &installed),
            SharedString::from(".SystemUIFont")
        );
    }

    #[test]
    fn validates_exact_installed_families_case_insensitively() {
        let installed = names(&["Noto Sans", "Noto Serif"]);
        assert!(family_available_in(&installed, "Noto Sans"));
        assert!(family_available_in(&installed, "noto serif"));
        assert!(!family_available_in(&installed, "Noto"));
        assert!(!family_available_in(&installed, "Missing Font"));
    }

    #[test]
    fn falls_back_to_the_system_face_without_the_mock_font() {
        let installed = names(&["Noto Sans"]);
        assert_eq!(
            first_installed(&["Noto Serif"], &installed),
            SharedString::from(".SystemUIFont")
        );
    }
}
