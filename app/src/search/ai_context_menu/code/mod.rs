pub mod data_source;
#[cfg(not(target_family = "wasm"))]
pub mod search_item;

#[cfg(not(target_family = "wasm"))]
use std::path::Path;

use warpui::AppContext;
#[cfg(not(target_family = "wasm"))]
use warpui::SingletonEntity;

#[cfg(not(target_family = "wasm"))]
use crate::ai::outline::{OutlineStatus, RepoOutlines};
#[cfg(not(target_family = "wasm"))]
use crate::workspace::ActiveSession;

/// Checks if the code symbols (outline) are currently being indexed for the active directory.
#[cfg(not(target_family = "wasm"))]
pub fn is_code_symbols_indexing(app: &AppContext) -> bool {
    let current_dir = app
        .windows()
        .state()
        .active_window
        .and_then(|window_id| ActiveSession::as_ref(app).path_if_local(window_id));
    current_dir.is_some_and(|current_dir| {
        RepoOutlines::as_ref(app)
            .get_outline(Path::new(current_dir))
            .is_some_and(|(status, _)| matches!(status, OutlineStatus::Pending))
    })
}

#[cfg(target_family = "wasm")]
#[allow(dead_code)]
pub fn is_code_symbols_indexing(_app: &AppContext) -> bool {
    false
}
