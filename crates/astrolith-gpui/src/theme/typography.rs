//! Application-owned typography for the writing desk.
//!
//! The shell keeps a quiet humanist interface voice; the document itself is set
//! in a serif so prose reads as a page, not as source code. One family is
//! chosen from installed fonts per process, because GPUI falls back to the
//! system UI face when a requested family is absent — which would silently undo
//! the whole design.

use std::sync::OnceLock;

use gpui_kit::{App, SharedString, component::ActiveTheme};

/// Comfortable line length for prose, in logical pixels at the base size.
pub(crate) const PROSE_MEASURE: f32 = 720.;

/// Horizontal inset inside the prose column.
pub(crate) const PROSE_INSET: f32 = 24.;

/// Vertical breathing room above the first line and below the last.
pub(crate) const PROSE_TOP: f32 = 64.;

/// Serif families tried in order, per platform.
const SERIF_ALTERNATES: &[&str] = if cfg!(target_os = "macos") {
    &[
        "New York",
        "Iowan Old Style",
        "Palatino",
        "Georgia",
        "Times New Roman",
    ]
} else if cfg!(target_os = "windows") {
    &["Georgia", "Cambria", "Constantia", "Times New Roman"]
} else {
    &[
        "Noto Serif",
        "Source Serif 4",
        "Source Serif Pro",
        "Liberation Serif",
        "DejaVu Serif",
        "Nimbus Roman",
        "Bitstream Vera Serif",
    ]
};

/// The virtual family GPUI resolves on every platform.
const SYSTEM_UI_FONT: &str = ".SystemUIFont";

/// The book face for the document.
pub(crate) fn prose_font(cx: &App) -> SharedString {
    static RESOLVED: OnceLock<SharedString> = OnceLock::new();
    RESOLVED
        .get_or_init(|| first_installed(SERIF_ALTERNATES, installed_font_names(cx)))
        .clone()
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
/// The literary default pairs the book serif with a sans interface. An explicit
/// interface family is used everywhere, including these headings, so the
/// setting means what it says.
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
    fn picks_the_first_installed_serif() {
        let installed = names(&["DejaVu Serif", "Noto Serif", ".SystemUIFont"]);
        assert_eq!(
            first_installed(&["Noto Serif", "DejaVu Serif"], &installed),
            SharedString::from("Noto Serif")
        );
        assert_eq!(
            first_installed(&["Source Serif 4", "DejaVu Serif"], &installed),
            SharedString::from("DejaVu Serif")
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
    fn falls_back_to_the_system_face_without_a_serif() {
        let installed = names(&["Noto Sans"]);
        assert_eq!(
            first_installed(&["Noto Serif"], &installed),
            SharedString::from(".SystemUIFont")
        );
    }
}
