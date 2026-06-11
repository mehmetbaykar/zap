//! `ProxyCredentials`: stores the proxy Basic Auth password in the OS keychain (see Issue #72).
//!
//! Only the password is stored; non-sensitive fields such as username and URL still live
//! in `NetworkSettings`' settings.toml. The design mirrors
//! `crate::ai::agent_providers::AgentProviderSecrets`: built on
//! `warpui_extras::secure_storage` (macOS Keychain / Windows DPAPI / Linux Keyring).
//!
//! Note: the proxy has only one global password, so storage holds a single key whose value
//! is the raw password string (no longer a JSON map).

use warpui::{Entity, ModelContext, SingletonEntity};
use warpui_extras::secure_storage::{self, AppContextExt};

const SECURE_STORAGE_KEY: &str = "ProxyPassword";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyCredentialsEvent {
    /// The password value changed (may be empty).
    PasswordChanged,
}

/// Singleton: manages the Basic Auth password for the global HTTP proxy.
pub struct ProxyCredentials {
    password: String,
}

impl ProxyCredentials {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        Self {
            password: Self::load_from_storage(ctx),
        }
    }

    /// Reads the current password; returns an empty string when there is no value.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Sets / updates the password. Passing an empty string is equivalent to deletion.
    pub fn set_password(&mut self, password: String, ctx: &mut ModelContext<Self>) {
        if self.password == password {
            return;
        }
        self.password = password;
        self.persist(ctx);
        ctx.emit(ProxyCredentialsEvent::PasswordChanged);
    }

    fn load_from_storage(ctx: &mut ModelContext<Self>) -> String {
        match ctx.secure_storage().read_value(SECURE_STORAGE_KEY) {
            Ok(value) => value,
            Err(secure_storage::Error::NotFound) => String::new(),
            Err(e) => {
                log::error!("Failed to read proxy password: {e:#}");
                String::new()
            }
        }
    }

    fn persist(&self, ctx: &mut ModelContext<Self>) {
        if self.password.is_empty() {
            // An empty string means "no password"; a failed delete is acceptable and only logged.
            // Avoid a let-chain (the app crate is Rust 2021) by checking in two steps.
            if let Err(e) = ctx.secure_storage().remove_value(SECURE_STORAGE_KEY) {
                if !matches!(e, secure_storage::Error::NotFound) {
                    log::error!("Failed to remove proxy password: {e:#}");
                }
            }
            return;
        }
        if let Err(e) = ctx
            .secure_storage()
            .write_value(SECURE_STORAGE_KEY, &self.password)
        {
            log::error!("Failed to write proxy password: {e:#}");
        }
    }
}

impl Entity for ProxyCredentials {
    type Event = ProxyCredentialsEvent;
}

impl SingletonEntity for ProxyCredentials {}
