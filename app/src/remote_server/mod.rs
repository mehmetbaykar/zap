// Re-export everything from the `remote_server` crate so existing
// `crate::remote_server::*` imports in `app` continue to work.
pub use remote_server::*;

#[cfg(not(target_family = "wasm"))]
pub mod codebase_index_model;
#[cfg(not(target_family = "wasm"))]
mod codebase_index_status;
pub mod diff_state_proto;
#[cfg(not(target_family = "wasm"))]
pub mod diff_state_tracker;
pub mod git_status_proto;
#[cfg(not(target_family = "wasm"))]
mod ripgrep_search;
#[cfg(not(target_family = "wasm"))]
pub mod server_buffer_tracker;
#[cfg(not(target_family = "wasm"))]
pub mod server_model;
#[cfg(not(target_family = "wasm"))]
pub mod ssh_transport;
#[cfg(unix)]
pub mod unix;

/// Run the `remote-server-proxy` subcommand.
#[cfg(unix)]
pub fn run_proxy(identity_key: String) -> anyhow::Result<()> {
    unix::proxy::run(&identity_key)
}

#[cfg(not(unix))]
pub fn run_proxy(identity_key: String) -> anyhow::Result<()> {
    let _ = identity_key;
    anyhow::bail!("remote-server-proxy is not supported on this platform")
}

/// Run the `remote-server-daemon` subcommand.
#[cfg(unix)]
pub fn run_daemon(identity_key: String) -> anyhow::Result<()> {
    unix::run_daemon(identity_key)
}

#[cfg(not(unix))]
pub fn run_daemon(identity_key: String) -> anyhow::Result<()> {
    let _ = identity_key;
    anyhow::bail!("remote-server-daemon is not supported on this platform")
}
