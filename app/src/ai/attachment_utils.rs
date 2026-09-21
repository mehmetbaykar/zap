use std::path::{Path, PathBuf};
use std::sync::Arc;

use session_sharing_protocol::common::AgentAttachment;
use warp_core::safe_warn;
use warp_errors::report_error;

/// Max file attachment size is 10 MB.
pub(crate) const MAX_ATTACHMENT_SIZE_BYTES: usize = 10 * 1024 * 1024;

/// Returns the per-session directory for downloading file attachments,
/// based on the agent's working directory.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn attachments_download_dir(working_dir: &Path) -> PathBuf {
    working_dir.join(".warp").join("attachments")
}
