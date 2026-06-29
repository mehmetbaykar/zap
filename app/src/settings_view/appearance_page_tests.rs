use super::{fallback_font_dropdown_should_include_font, FontType};
use crate::settings::{MonospaceFallbackFontName, DEFAULT_MONOSPACE_FONT_NAME};
use settings::Setting as _;

#[test]
fn fallback_font_dropdown_includes_default_monospace_font() {
    assert_eq!(MonospaceFallbackFontName::default_value(), "");
    assert!(fallback_font_dropdown_should_include_font(
        DEFAULT_MONOSPACE_FONT_NAME,
        FontType::Monospace,
        FontType::Monospace,
        "",
    ));
}
