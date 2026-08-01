//! The headless `warp-tui` front-end's app-side entry point.
//!
//! `warp_tui` boots the real local app via [`crate::run_tui`]. Once shared
//! initialization is complete, [`init`] mounts the TUI directly; the fork has
//! no Warp account or device-authorization gate.

use warpui::AppContext;

use crate::TuiMountFn;
use crate::ai::mcp::FileBasedMCPManager;
use crate::auth::AuthStateProvider;
use crate::auth::auth_manager::{AuthManager, AuthManagerEvent};

/// Mounts the local TUI after the shared headless app state is initialized.
pub(crate) fn init(mount: TuiMountFn, ctx: &mut AppContext) {
    mount(ctx);
}
