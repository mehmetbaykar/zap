//! Global HTTP proxy configuration.
//!
//! See Issue #72: Zap needs a globally configurable proxy setting that uniformly
//! covers all outbound HTTP requests (BYOP model list fetching, autoupdate,
//! conversation loading, etc.).
//!
//! Design points:
//! - Three modes for [`ProxyMode`]: `System` / `Custom` / `Off`.
//! - `System` falls back to reqwest's default behavior; the workspace's reqwest
//!   already enables the `system-proxy` + `macos-system-configuration` features,
//!   so macOS reads SystemConfiguration, Windows reads WinINET, Linux reads
//!   `HTTP_PROXY` and similar environment variables -- no need to implement it
//!   ourselves.
//! - `Custom` explicitly specifies the URL / basic auth / no_proxy list.
//! - `Off` calls [`reqwest::ClientBuilder::no_proxy`], fully disabling the proxy
//!   (including environment variables).
//!
//! The application injects the config via [`set_global_proxy_config`] at startup
//! and on settings changes; all subsequent [`crate::Client::new`] calls read this
//! global value and apply it to reqwest.
//!
//! reqwest does not support runtime proxy switching on an already-constructed
//! `Client`, so callers must rebuild the Client instance after changing settings
//! (e.g. `AutoupdateState::new(http_client::Client::new())`).

use std::sync::{OnceLock, RwLock};

/// Global proxy mode.
///
/// The default is `Off`: this avoids a `Client` constructed during cold start --
/// before the app-layer settings have been injected -- picking up an unexpected
/// system proxy detected by reqwest. app::ProxyMode shares the same default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProxyMode {
    /// Disable the proxy, including environment variables. The default.
    #[default]
    Off,
    /// Fully follow the system / environment variables (reqwest's default behavior).
    System,
    /// Use the proxy explicitly configured in [`ProxyConfig::url`].
    Custom,
}

impl ProxyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ProxyMode::System => "system",
            ProxyMode::Custom => "custom",
            ProxyMode::Off => "off",
        }
    }

    pub fn from_str_lenient(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "system" => ProxyMode::System,
            "custom" => ProxyMode::Custom,
            // off / disabled / none / unknown all fall back to Off (the default), avoiding an accidental system proxy.
            _ => ProxyMode::Off,
        }
    }
}

/// The resolved global proxy configuration.
///
/// `username` is stored in plaintext in settings.toml, while `password` is stored
/// separately via `managed_secrets` (same pattern as the BYOP API key); the caller
/// injects it into [`Self::password`] before assembling this struct.
#[derive(Clone, Debug, Default)]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    /// E.g. `http://proxy.corp:8080`. Only takes effect under [`ProxyMode::Custom`].
    pub url: String,
    pub username: String,
    pub password: String,
    /// Comma-separated list of hosts; an empty string means no exceptions.
    pub no_proxy: String,
}

impl ProxyConfig {
    /// Applies this configuration to a `reqwest::ClientBuilder`.
    ///
    /// On error (`Custom` mode but an invalid URL) it warns in the log and falls
    /// back to reqwest's default behavior, rather than letting `Client::new()` panic.
    pub fn apply(&self, mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        match self.mode {
            ProxyMode::System => builder,
            ProxyMode::Off => builder.no_proxy(),
            ProxyMode::Custom => {
                let trimmed = self.url.trim();
                if trimmed.is_empty() {
                    log::warn!(
                        "HTTP proxy is set to Custom but the URL is empty; falling back to reqwest default (reads system proxy)"
                    );
                    return builder;
                }

                let proxy_result = reqwest::Proxy::all(trimmed);
                let mut proxy = match proxy_result {
                    Ok(p) => p,
                    Err(err) => {
                        log::warn!(
                            "HTTP proxy URL '{trimmed}' is invalid ({err}); falling back to reqwest default"
                        );
                        return builder;
                    }
                };

                if !self.username.is_empty() || !self.password.is_empty() {
                    proxy = proxy.basic_auth(&self.username, &self.password);
                }

                if !self.no_proxy.trim().is_empty() {
                    if let Some(no_proxy) = reqwest::NoProxy::from_string(self.no_proxy.trim()) {
                        proxy = proxy.no_proxy(Some(no_proxy));
                    }
                }

                builder = builder.proxy(proxy);
                builder
            }
        }
    }
}

static GLOBAL_PROXY_CONFIG: OnceLock<RwLock<ProxyConfig>> = OnceLock::new();

fn slot() -> &'static RwLock<ProxyConfig> {
    GLOBAL_PROXY_CONFIG.get_or_init(|| RwLock::new(ProxyConfig::default()))
}

/// Installs a new global proxy configuration.
///
/// Only affects `Client`s constructed after this call. A `reqwest::Client` cannot
/// switch proxies once constructed, so the application layer needs to rebuild all
/// shared Client instances after changing settings.
pub fn set_global_proxy_config(cfg: ProxyConfig) {
    let lock = slot();
    if let Ok(mut guard) = lock.write() {
        *guard = cfg;
    } else {
        log::error!("Failed to write global HTTP proxy configuration: RwLock is poisoned");
    }
}

/// Reads the current global proxy configuration (returns the default if unset).
pub fn current_proxy_config() -> ProxyConfig {
    let lock = slot();
    match lock.read() {
        Ok(guard) => guard.clone(),
        Err(err) => {
            log::error!(
                "Failed to read global HTTP proxy configuration: RwLock is poisoned ({err})"
            );
            ProxyConfig::default()
        }
    }
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
