//! The headless `warp-tui` front-end's app-side entry point.
//!
//! `warp_tui` boots the real local app via [`crate::run_tui`]. Once shared
//! initialization is complete, [`init`] mounts the TUI directly; the fork has
//! no Warp account or device-authorization gate.

use warpui::AppContext;

use crate::TuiMountFn;

/// Mounts the local TUI after the shared headless app state is initialized.
pub(crate) fn init(mount: TuiMountFn, ctx: &mut AppContext) {
    mount(ctx);
}

