//! Small presentation helpers for the `warp-tui` front-end's TUI views.
use std::time::Duration;

use warpui_core::elements::animation::AnimationClock;
use warpui_core::elements::tui::{
    Modifier, TuiConstrainedBox, TuiElement, TuiFlex, TuiStyle, TuiText,
};
use warpui_core::elements::CrossAxisAlignment;
use warpui_core::AppContext;

use crate::tui_builder::TuiUiBuilder;
use crate::warping_indicator::render_spinner;

/// Abbreviates a leading home-directory prefix of `path` to `~`.
pub(crate) fn abbreviate_home_prefix(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(&*home) {
            if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') {
                return format!("~{rest}");
            }
        }
    }
    path.to_owned()
}

/// Compacts a path for the one-line session footer while preserving its root
/// (or first relative component) and basename.
pub(crate) fn compact_footer_path(path: &str) -> String {
    let path = abbreviate_home_prefix(path);
    let separator = if path.contains('\\') && !path.contains('/') {
        '\\'
    } else {
        '/'
    };
    let components: Vec<_> = path
        .split(separator)
        .filter(|component| !component.is_empty())
        .collect();
    if components.len() <= 2 {
        return path;
    }

    let last = components
        .last()
        .expect("path has more than two components");
    if path.starts_with(separator) {
        format!("{separator}…{separator}{last}")
    } else {
        format!(
            "{}{separator}…{separator}{last}",
            components.first().expect("path has components")
        )
    }
}

/// Vertically centers `content` with its existing horizontal alignment.
fn vertically_centered(content: TuiFlex) -> Box<dyn TuiElement> {
    TuiFlex::column()
        .flex_child(TuiFlex::column().finish())
        .child(content.finish())
        .flex_child(TuiFlex::column().finish())
        .finish()
}

/// Placeholder shown while the terminal session is being created.
pub(crate) fn terminal_starting() -> Box<dyn TuiElement> {
    let dim = TuiStyle::default().add_modifier(Modifier::DIM);
    vertically_centered(
        TuiFlex::column().child(
            TuiText::new("Starting terminal…")
                .with_style(dim)
                .truncate()
                .finish(),
        ),
    )
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
