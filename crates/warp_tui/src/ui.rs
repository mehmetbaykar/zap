//! Small presentation helpers for the `warp-tui` front-end's TUI views.

use warpui_core::elements::tui::{Modifier, TuiElement, TuiFlex, TuiStyle, TuiText};

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

/// Vertically centers `content` by padding above and below with flex spacers.
pub(crate) fn centered(content: TuiFlex) -> Box<dyn TuiElement> {
    TuiFlex::column()
        .flex_child(TuiFlex::column().finish())
        .child(content.finish())
        .flex_child(TuiFlex::column().finish())
        .finish()
}

/// Placeholder shown while the terminal session is being created.
pub(crate) fn terminal_starting() -> Box<dyn TuiElement> {
    let dim = TuiStyle::default().add_modifier(Modifier::DIM);
    centered(
        TuiFlex::column().child(
            TuiText::new("Starting terminal…")
                .with_style(dim)
                .truncate()
                .finish(),
        ),
    )
}
