//! OneKey credential loading: reads all saved server credentials from the SSH
//! Manager persistence layer + Keychain/DPAPI/Linux Keyring, so `TerminalView`
//! can pop up a selection menu when it detects a PTY password prompt.
//!
//! ## Note
//!
//! - It internally calls `warp_ssh_manager::with_conn` (synchronous Mutex +
//!   SQLite) and `KeychainSecretStore::get` (synchronous OS API), which **must
//!   not** be called synchronously on the UI main thread directly — it stutters
//!   once there are many servers. The caller must go through
//!   `tokio::task::spawn_blocking`.
//! - The secret is held in `Zeroizing<String>` throughout and zeroed automatically when dropped.

use anyhow::Result;
use zeroize::Zeroizing;

use warp_ssh_manager::{
    AuthType, KeychainSecretStore, NodeKind, SecretKind, SshRepository, SshSecretStore,
};

pub struct OneKeyCredential {
    pub label: String,
    pub subtitle: String,
    pub secret: Zeroizing<String>,
}

pub fn load_saved_ssh_credentials() -> Result<Vec<OneKeyCredential>> {
    let store = KeychainSecretStore;
    warp_ssh_manager::with_conn(|conn| {
        let nodes = SshRepository::list_nodes(conn)?;
        let mut credentials = Vec::new();

        for node in nodes {
            if node.kind != NodeKind::Server {
                continue;
            }
            let Some(server) = SshRepository::get_server(conn, &node.id)? else {
                continue;
            };
            let kind = match server.auth_type {
                AuthType::Password => SecretKind::Password,
                AuthType::Key => SecretKind::Passphrase,
            };
            let secret = match store.get(&node.id, kind) {
                Ok(Some(secret)) if !secret.is_empty() => secret,
                Ok(Some(_)) | Ok(None) => continue,
                Err(e) => {
                    log::warn!("onekey: failed to read saved ssh credential: {e}");
                    continue;
                }
            };
            let target = if server.username.is_empty() {
                format!("{}:{}", server.host, server.port)
            } else {
                format!("{}@{}:{}", server.username, server.host, server.port)
            };
            // kind is derived from auth_type, so it can only be Password or
            // Passphrase; RootPassword is not in OneKey's own scope (it goes
            // through the separate su confirmation popup flow).
            let subtitle = match server.auth_type {
                AuthType::Password => target,
                AuthType::Key => {
                    let key_path = server.key_path.as_deref().unwrap_or("key");
                    format!("{key_path} for {target}")
                }
            };
            credentials.push(OneKeyCredential {
                label: node.name,
                subtitle,
                secret,
            });
        }

        Ok(credentials)
    })
}
