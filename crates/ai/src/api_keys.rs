use std::collections::{HashMap, HashSet};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, SystemTime};

#[cfg(not(target_family = "wasm"))]
use futures::channel::oneshot;
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;
use warp_errors::report_error;
use warpui_core::{Entity, ModelContext, SingletonEntity};
use warpui_extras::secure_storage::{self, AppContextExt};

pub use crate::aws_credentials::{AwsCredentials, AwsCredentialsState};

const SECURE_STORAGE_KEY: &str = "AiApiKeys";
const CUSTOM_ENDPOINT_KEYS_SECURE_STORAGE_KEY: &str = "AiCustomEndpointKeys";
const GENERATED_ENDPOINT_PREFIX: &str = "endpoint-";
const LEGACY_ENDPOINT_PREFIX: &str = "legacy-";

/// Secure-storage key for the connected xAI/Grok subscription's OAuth tokens.
/// Kept separate from [`SECURE_STORAGE_KEY`] because these are OAuth tokens with
/// a refresh lifecycle, not a user-pasted static key.
const GROK_SECURE_STORAGE_KEY: &str = "GrokOAuthTokens";

/// Emitted when user-provided API keys are updated in-memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyManagerEvent {
    KeysUpdated,
}

/// User-provided API keys for AI providers.
///
/// These are used for "Bring Your Own API Key" functionality, allowing
/// users to use their own API keys instead of Zap's.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiKeys {
    pub google: Option<String>,
    pub anthropic: Option<String>,
    pub openai: Option<String>,
    pub open_router: Option<String>,
    pub custom_endpoints: Vec<CustomEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomEndpoint {
    pub name: String,
    pub url: String,
    pub api_key: String,
    pub models: Vec<CustomEndpointModel>,
    pub schema: CustomEndpointSchema,
}

/// The request/response protocol used by a custom inference endpoint.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CustomEndpointSchema {
    /// OpenAI Chat Completions, retained as the legacy/default protocol.
    #[default]
    OpenaiChatCompletions,
    /// OpenAI Responses.
    OpenaiResponses,
    /// Anthropic Messages.
    AnthropicMessages,
}

impl CustomEndpointSchema {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::OpenaiChatCompletions => "OpenAI Chat Completions",
            Self::OpenaiResponses => "OpenAI Responses",
            Self::AnthropicMessages => "Anthropic Messages",
        }
    }

    pub fn from_display_name(name: &str) -> Option<Self> {
        match name {
            "OpenAI Chat Completions" => Some(Self::OpenaiChatCompletions),
            "OpenAI Responses" => Some(Self::OpenaiResponses),
            "Anthropic Messages" => Some(Self::AnthropicMessages),
            _ => None,
        }
    }
}

/// Stable file-safe identity for a custom endpoint definition.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct CustomEndpointId(String);

impl CustomEndpointId {
    pub fn generated() -> Self {
        Self(format!("{GENERATED_ENDPOINT_PREFIX}{}", Uuid::new_v4()))
    }

    pub fn from_legacy(endpoint: &CustomEndpoint, index: usize) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(index.to_le_bytes());
        hasher.update(endpoint.name.as_bytes());
        hasher.update([0]);
        hasher.update(endpoint.url.as_bytes());
        for model in &endpoint.models {
            hasher.update([0]);
            hasher.update(model.config_key.as_bytes());
        }
        let digest = hex::encode(hasher.finalize());
        Self(format!("{LEGACY_ENDPOINT_PREFIX}{}", &digest[..24]))
    }

    pub fn parse(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
        .then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CustomEndpointId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).ok_or_else(|| serde::de::Error::custom("invalid custom endpoint key"))
    }
}

impl fmt::Display for CustomEndpointId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Non-secret custom endpoint configuration stored in `settings.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomEndpointDefinition {
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub schema: CustomEndpointSchema,
    pub models: Vec<CustomEndpointModel>,
}

impl CustomEndpointDefinition {
    pub fn from_legacy(endpoint: &CustomEndpoint) -> Self {
        Self {
            name: endpoint.name.clone(),
            base_url: endpoint.url.clone(),
            schema: endpoint.schema,
            models: endpoint.models.clone(),
        }
    }

    pub fn into_endpoint(self, api_key: String) -> CustomEndpoint {
        CustomEndpoint {
            name: self.name,
            url: self.base_url,
            api_key,
            models: self.models,
            schema: self.schema,
        }
    }

    fn is_valid(&self) -> bool {
        !self.name.trim().is_empty()
            && validate_custom_endpoint_url(&self.base_url).is_ok()
            && !self.models.is_empty()
            && self.models.iter().all(|model| {
                !model.name.trim().is_empty()
                    && !model.config_key.trim().is_empty()
                    && model
                        .alias
                        .as_deref()
                        .is_none_or(|alias| !alias.trim().is_empty())
            })
    }
}

/// Complete ordered collection of custom endpoint definitions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CustomEndpointDefinitions(IndexMap<CustomEndpointId, CustomEndpointDefinition>);

impl CustomEndpointDefinitions {
    pub fn definitions(
        &self,
    ) -> impl Iterator<Item = (&CustomEndpointId, &CustomEndpointDefinition)> {
        self.0.iter()
    }

    pub fn get(&self, id: &CustomEndpointId) -> Option<&CustomEndpointDefinition> {
        self.0.get(id)
    }

    pub fn id_at(&self, index: usize) -> Option<&CustomEndpointId> {
        self.0.get_index(index).map(|(id, _)| id)
    }

    pub fn insert(
        &mut self,
        id: CustomEndpointId,
        definition: CustomEndpointDefinition,
    ) -> anyhow::Result<Option<CustomEndpointDefinition>> {
        let mut definitions = self.0.clone();
        let previous = definitions.insert(id, definition);
        *self = Self::validated(definitions)
            .ok_or_else(|| anyhow::anyhow!("invalid custom endpoint definition"))?;
        Ok(previous)
    }

    pub fn remove(&mut self, id: &CustomEndpointId) -> Option<CustomEndpointDefinition> {
        self.0.shift_remove(id)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn from_legacy(
        endpoints: &[CustomEndpoint],
    ) -> anyhow::Result<(Self, HashMap<CustomEndpointId, String>)> {
        let mut definitions = IndexMap::new();
        let mut keys = HashMap::new();
        for (index, endpoint) in endpoints.iter().enumerate() {
            let id = CustomEndpointId::from_legacy(endpoint, index);
            definitions.insert(id.clone(), CustomEndpointDefinition::from_legacy(endpoint));
            if !endpoint.api_key.trim().is_empty() {
                keys.insert(id, endpoint.api_key.clone());
            }
        }
        let definitions = Self::validated(definitions)
            .ok_or_else(|| anyhow::anyhow!("legacy custom endpoints are invalid"))?;
        Ok((definitions, keys))
    }

    fn validated(
        definitions: IndexMap<CustomEndpointId, CustomEndpointDefinition>,
    ) -> Option<Self> {
        let mut config_keys = HashSet::new();
        if definitions.values().all(CustomEndpointDefinition::is_valid)
            && definitions
                .values()
                .flat_map(|definition| &definition.models)
                .all(|model| config_keys.insert(model.config_key.clone()))
        {
            Some(Self(definitions))
        } else {
            None
        }
    }
}

impl<'de> Deserialize<'de> for CustomEndpointDefinitions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let definitions =
            IndexMap::<CustomEndpointId, CustomEndpointDefinition>::deserialize(deserializer)?;
        Self::validated(definitions)
            .ok_or_else(|| serde::de::Error::custom("invalid custom endpoint definitions"))
    }
}

impl settings_value::SettingsValue for CustomEndpointDefinitions {
    fn from_file_value(value: &serde_json::Value) -> Option<Self> {
        let definitions = serde_json::from_value::<
            IndexMap<CustomEndpointId, CustomEndpointDefinition>,
        >(value.clone())
        .ok()?;
        Self::validated(definitions)
    }

    fn file_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema
    where
        Self: schemars::JsonSchema,
    {
        generator.subschema_for::<HashMap<String, CustomEndpointDefinition>>()
    }
}

impl schemars::JsonSchema for CustomEndpointDefinitions {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("CustomEndpointDefinitions")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<HashMap<String, CustomEndpointDefinition>>()
    }
}

pub fn validate_custom_endpoint_url(value: &str) -> Result<(), &'static str> {
    let parsed = Url::parse(value).map_err(|_| "Invalid URL")?;
    if parsed.scheme() != "https" {
        return Err("URL must use HTTPS");
    }
    let Some(host) = parsed.host_str().filter(|host| !host.is_empty()) else {
        return Err("URL must include a host");
    };
    if is_restricted_host(host) {
        return Err("URL must not use a local or private host");
    }
    Ok(())
}

fn is_restricted_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok_and(is_restricted_ip)
}

fn is_restricted_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_restricted_ipv4(ip),
        IpAddr::V6(ip) => is_restricted_ipv6(ip),
    }
}

fn is_restricted_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_unspecified() || ip.is_private() || ip.is_link_local()
}

fn is_restricted_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.segments()[0] & 0xfe00 == 0xfc00
        || ip.segments()[0] & 0xffc0 == 0xfe80
        || ip.to_ipv4_mapped().is_some_and(is_restricted_ipv4)
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct CustomEndpointModel {
    pub name: String,
    pub alias: Option<String>,
    /// Stable local identifier for this endpoint model.
    pub config_key: String,
}

impl CustomEndpointModel {
    /// Picker label: prefer the user-provided alias; fall back to the raw model name.
    pub fn display_label(&self) -> &str {
        match self.alias.as_deref() {
            Some(alias) if !alias.trim().is_empty() => alias,
            _ => &self.name,
        }
    }
}

impl ApiKeys {
    pub fn has_any_key(&self) -> bool {
        self.openai.is_some()
            || self.anthropic.is_some()
            || self.google.is_some()
            || self.open_router.is_some()
            || self
                .custom_endpoints
                .iter()
                .any(|endpoint| !endpoint.api_key.trim().is_empty())
    }

    pub fn has_custom_endpoints(&self) -> bool {
        !self.custom_endpoints.is_empty()
    }

    /// Number of single-provider API keys currently configured (OpenAI,
    /// Anthropic, Google, OpenRouter). Custom endpoints are counted separately
    /// via `custom_endpoints`.
    pub fn provider_key_count(&self) -> usize {
        [
            &self.openai,
            &self.anthropic,
            &self.google,
            &self.open_router,
        ]
        .into_iter()
        .filter(|key| key.as_deref().is_some_and(|v| !v.trim().is_empty()))
        .count()
    }
}

/// OAuth tokens for a connected xAI / Grok subscription (e.g. SuperGrok).
///
/// Persisted to secure storage under [`GROK_SECURE_STORAGE_KEY`], separate from
/// the BYO [`ApiKeys`] blob because these are OAuth tokens with a refresh
/// lifecycle rather than a user-pasted static key. `crate::grok_subscription`
/// owns refreshing them; this module is the local storage source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GrokTokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Absolute time at which `access_token` expires, if the provider told us.
    #[serde(default)]
    pub expires_at: Option<SystemTime>,
    /// When the user originally connected the subscription (i.e. when the
    /// browser OAuth flow completed). Carried over across token refreshes so
    /// it keeps reflecting the initial connection, not the latest refresh;
    /// surfaced in the settings UI as "Connected on ...". `None` for tokens
    /// stored before this field existed.
    #[serde(default)]
    pub connected_at: Option<SystemTime>,
}

impl GrokTokens {
    /// Returns the access token whenever it is non-empty, regardless of
    /// expiry. `crate::grok_subscription` refreshes (nearly) expired tokens in
    /// the background before provider-direct use.
    pub fn access_token_for_request(&self) -> Option<&str> {
        (!self.access_token.trim().is_empty()).then_some(self.access_token.as_str())
    }

    /// Returns `true` when the token is known to expire within `lead_time` and
    /// should be proactively refreshed. Tokens with an unknown expiry never
    /// report as needing a refresh (there's no expiry signal to act on).
    pub fn needs_refresh(&self, lead_time: Duration) -> bool {
        match self.expires_at {
            Some(expires_at) => expires_at <= SystemTime::now() + lead_time,
            None => false,
        }
    }

    /// Returns `true` when the token is known to be at or past its hard expiry.
    /// Unlike [`Self::needs_refresh`] there is no lead time: a token expiring
    /// soon but still valid reports `false`. Tokens with an unknown expiry are
    /// never considered expired.
    pub fn is_expired(&self) -> bool {
        self.needs_refresh(Duration::ZERO)
    }
}

/// Outcome of a Grok OAuth token refresh, delivered to each request blocked
/// waiting on it so the request can either send with the freshly refreshed
/// token or surface the failure instead of sending an expired one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrokRefreshOutcome {
    /// The token was refreshed and the new value stored.
    Refreshed,
    /// The refresh failed; the stored token is unchanged (still expired).
    Failed,
}

struct CustomEndpointState {
    definitions: Option<CustomEndpointDefinitions>,
    settings_valid: bool,
    keys: HashMap<CustomEndpointId, String>,
    resolved: Vec<CustomEndpoint>,
}

/// A structure that manages API keys for AI providers.
pub struct ApiKeyManager {
    keys: ApiKeys,
    custom_endpoints: CustomEndpointState,
    /// OAuth tokens for a connected xAI/Grok subscription, if any. Persisted
    /// separately from `keys` under [`GROK_SECURE_STORAGE_KEY`];
    /// `crate::grok_subscription` keeps these fresh.
    grok_tokens: Option<GrokTokens>,
    /// Whether background refresh of `grok_tokens` is currently allowed.
    /// Mirrors the BYO API key policy, which lives in the app layer; wired in
    /// via `ApiKeyManager::set_grok_refresh_allowed` (`crate::grok_subscription`).
    #[cfg(not(target_family = "wasm"))]
    pub(crate) grok_refresh_allowed: bool,
    /// Coordinates Grok token refreshes so only one runs at a time (shared by
    /// the proactive refresh timer and the request-time blocking refresh in
    /// `crate::grok_subscription`). `Some` means a refresh is in flight; the
    /// vector holds the completion senders for any requests waiting on it (it
    /// may be empty for a proactive refresh with no waiters). `None` means no
    /// refresh is running. Always cleared when the refresh finishes.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) grok_refresh_waiters: Option<Vec<oneshot::Sender<GrokRefreshOutcome>>>,
    pub(crate) aws_credentials_state: AwsCredentialsState,
    secure_storage_write_version: u64,
    grok_secure_storage_write_version: u64,
}

#[derive(Clone)]
pub struct CustomEndpointParams {
    pub name: String,
    pub url: String,
    pub api_key: String,
    pub models: Vec<(String, Option<String>, Option<String>)>,
    pub schema: CustomEndpointSchema,
}

impl ApiKeyManager {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let keys = Self::load_keys_from_secure_storage(ctx);
        let custom_endpoint_keys = Self::load_custom_endpoint_keys_from_secure_storage(ctx);
        let resolved_custom_endpoints = keys.custom_endpoints.clone();
        let grok_tokens = Self::load_grok_tokens_from_secure_storage(ctx);
        Self {
            keys,
            custom_endpoints: CustomEndpointState {
                definitions: None,
                settings_valid: true,
                keys: custom_endpoint_keys,
                resolved: resolved_custom_endpoints,
            },
            grok_tokens,
            #[cfg(not(target_family = "wasm"))]
            grok_refresh_allowed: false,
            #[cfg(not(target_family = "wasm"))]
            grok_refresh_waiters: None,
            aws_credentials_state: AwsCredentialsState::Missing,
            secure_storage_write_version: 0,
            grok_secure_storage_write_version: 0,
        }
    }

    pub fn keys(&self) -> &ApiKeys {
        &self.keys
    }

    pub fn custom_endpoints(&self) -> &[CustomEndpoint] {
        &self.custom_endpoints.resolved
    }

    pub fn custom_endpoint_definitions(&self) -> Option<&CustomEndpointDefinitions> {
        self.custom_endpoints.definitions.as_ref()
    }

    pub fn custom_endpoint_settings_valid(&self) -> bool {
        self.custom_endpoints.settings_valid
    }

    pub fn custom_endpoint_key(&self, id: &CustomEndpointId) -> Option<&str> {
        self.custom_endpoints
            .keys
            .get(id)
            .map(String::as_str)
            .filter(|key| !key.trim().is_empty())
    }

    pub fn set_custom_endpoint_definitions(
        &mut self,
        definitions: CustomEndpointDefinitions,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.custom_endpoints.settings_valid
            && self.custom_endpoints.definitions.as_ref() == Some(&definitions)
        {
            return;
        }
        self.custom_endpoints.settings_valid = true;
        self.custom_endpoints.definitions = Some(definitions);
        self.rebuild_custom_endpoints();
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
    }

    pub fn invalidate_custom_endpoint_definitions(&mut self, ctx: &mut ModelContext<Self>) {
        if !self.custom_endpoints.settings_valid {
            return;
        }
        self.custom_endpoints.settings_valid = false;
        self.custom_endpoints.definitions = Some(CustomEndpointDefinitions::default());
        self.custom_endpoints.resolved.clear();
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
    }

    pub fn persist_custom_endpoint_key(
        &mut self,
        id: CustomEndpointId,
        key: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<()> {
        let mut keys = self.custom_endpoints.keys.clone();
        match key.filter(|key| !key.trim().is_empty()) {
            Some(key) => {
                keys.insert(id, key);
            }
            None => {
                keys.remove(&id);
            }
        }
        self.persist_custom_endpoint_keys(keys, ctx)
    }

    pub fn persist_custom_endpoint_keys(
        &mut self,
        keys: HashMap<CustomEndpointId, String>,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<()> {
        let json = serde_json::to_string(&keys).map_err(|error| {
            anyhow::Error::new(error).context("Failed to serialize custom endpoint keys")
        })?;
        ctx.secure_storage()
            .write_value(CUSTOM_ENDPOINT_KEYS_SECURE_STORAGE_KEY, &json)
            .map_err(|error| {
                anyhow::Error::new(error)
                    .context("Failed to write custom endpoint keys to secure storage")
            })?;
        if self.custom_endpoints.keys != keys {
            self.custom_endpoints.keys = keys;
            self.rebuild_custom_endpoints();
            ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        }
        Ok(())
    }

    /// Reloads API keys after another process updates the active secure-storage namespace.
    ///
    /// GUI edits mutate this manager directly before persisting, so they do not
    /// need to reload. TUI setup commands run in a separate process and notify
    /// the live TUI to refresh its cached keys after a successful write.
    pub fn reload_keys_from_secure_storage(&mut self, ctx: &mut ModelContext<Self>) {
        let keys = Self::load_keys_from_secure_storage(ctx);
        let custom_endpoint_keys = Self::load_custom_endpoint_keys_from_secure_storage(ctx);
        if self.keys == keys && self.custom_endpoints.keys == custom_endpoint_keys {
            return;
        }
        self.keys = keys;
        self.custom_endpoints.keys = custom_endpoint_keys;
        self.rebuild_custom_endpoints();
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
    }

    /// The currently stored xAI/Grok OAuth tokens, if the user has connected a
    /// Grok subscription.
    pub fn grok_tokens(&self) -> Option<&GrokTokens> {
        self.grok_tokens.as_ref()
    }

    /// Returns `true` when provider-direct Grok OAuth is connected with a usable
    /// access token.
    pub fn has_grok_subscription(&self) -> bool {
        self.grok_tokens
            .as_ref()
            .and_then(GrokTokens::access_token_for_request)
            .is_some()
    }

    /// Returns `true` when the user has any usable local BYO credential.
    pub fn has_any_key(&self) -> bool {
        self.keys.provider_key_count() > 0
            || self
                .custom_endpoints
                .resolved
                .iter()
                .any(|endpoint| !endpoint.api_key.trim().is_empty())
            || self.has_grok_subscription()
    }

    /// Stores (or clears, with `None`) the xAI/Grok OAuth tokens and persists
    /// them to secure storage. No-op when the value is unchanged so we don't
    /// emit spurious events or schedule redundant keychain writes.
    pub fn set_grok_tokens(&mut self, tokens: Option<GrokTokens>, ctx: &mut ModelContext<Self>) {
        if self.grok_tokens == tokens {
            return;
        }
        self.grok_tokens = tokens;
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_grok_tokens_to_secure_storage(ctx);
    }

    pub fn set_google_key(&mut self, key: Option<String>, ctx: &mut ModelContext<Self>) {
        self.keys.google = key;
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    pub fn set_anthropic_key(&mut self, key: Option<String>, ctx: &mut ModelContext<Self>) {
        self.keys.anthropic = key;
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    pub fn set_openai_key(&mut self, key: Option<String>, ctx: &mut ModelContext<Self>) {
        self.keys.openai = key;
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    pub fn set_open_router_key(&mut self, key: Option<String>, ctx: &mut ModelContext<Self>) {
        self.keys.open_router = key;
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    pub fn add_custom_endpoint(
        &mut self,
        params: CustomEndpointParams,
        ctx: &mut ModelContext<Self>,
    ) {
        let CustomEndpointParams {
            name,
            url,
            api_key,
            models,
            schema,
        } = params;
        self.keys.custom_endpoints.push(CustomEndpoint {
            name,
            url,
            api_key,
            schema,
            models: Self::custom_endpoint_models(models),
        });
        if self.custom_endpoints.definitions.is_none() {
            self.custom_endpoints.resolved = self.keys.custom_endpoints.clone();
        }
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    pub fn save_custom_endpoint(
        &mut self,
        index: usize,
        params: CustomEndpointParams,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(endpoint) = self.keys.custom_endpoints.get_mut(index) else {
            return;
        };
        let CustomEndpointParams {
            name,
            url,
            api_key,
            models,
            schema,
        } = params;
        *endpoint = CustomEndpoint {
            name,
            url,
            api_key,
            schema,
            models: Self::custom_endpoint_models(models),
        };
        if self.custom_endpoints.definitions.is_none() {
            self.custom_endpoints.resolved = self.keys.custom_endpoints.clone();
        }
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    pub fn remove_custom_endpoint(&mut self, index: usize, ctx: &mut ModelContext<Self>) {
        if index >= self.keys.custom_endpoints.len() {
            return;
        }
        self.keys.custom_endpoints.remove(index);
        if self.custom_endpoints.definitions.is_none() {
            self.custom_endpoints.resolved = self.keys.custom_endpoints.clone();
        }
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    fn custom_endpoint_models(
        models: Vec<(String, Option<String>, Option<String>)>,
    ) -> Vec<CustomEndpointModel> {
        models
            .into_iter()
            .map(|(name, alias, config_key)| CustomEndpointModel {
                name,
                alias,
                config_key: config_key
                    .filter(|key| !key.is_empty())
                    .unwrap_or_else(|| Uuid::new_v4().to_string()),
            })
            .collect()
    }

    pub fn clear_custom_endpoints(&mut self, ctx: &mut ModelContext<Self>) {
        if self.keys.custom_endpoints.is_empty() {
            return;
        }
        self.keys.custom_endpoints.clear();
        if self.custom_endpoints.definitions.is_none() {
            self.custom_endpoints.resolved.clear();
        }
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
        self.write_keys_to_secure_storage(ctx);
    }

    pub fn set_aws_credentials_state(
        &mut self,
        state: AwsCredentialsState,
        ctx: &mut ModelContext<Self>,
    ) {
        self.aws_credentials_state = state;
        ctx.emit(ApiKeyManagerEvent::KeysUpdated);
    }

    pub fn aws_credentials_state(&self) -> &AwsCredentialsState {
        &self.aws_credentials_state
    }

    fn load_keys_from_secure_storage(ctx: &mut ModelContext<Self>) -> ApiKeys {
        let key_json = match ctx.secure_storage().read_value(SECURE_STORAGE_KEY) {
            Ok(json) => json,
            Err(e) => {
                if !matches!(e, secure_storage::Error::NotFound) {
                    report_error!(
                        anyhow::Error::new(e)
                            .context("Failed to read API keys from secure storage")
                    );
                }
                return ApiKeys::default();
            }
        };

        match serde_json::from_str(&key_json) {
            Ok(keys) => keys,
            Err(e) => {
                report_error!(anyhow::Error::new(e).context("Failed to deserialize API keys"));
                ApiKeys::default()
            }
        }
    }

    fn load_custom_endpoint_keys_from_secure_storage(
        ctx: &mut ModelContext<Self>,
    ) -> HashMap<CustomEndpointId, String> {
        let json = match ctx
            .secure_storage()
            .read_value(CUSTOM_ENDPOINT_KEYS_SECURE_STORAGE_KEY)
        {
            Ok(json) => json,
            Err(error) => {
                if !matches!(error, secure_storage::Error::NotFound) {
                    report_error!(
                        anyhow::Error::new(error)
                            .context("Failed to read custom endpoint keys from secure storage")
                    );
                }
                return HashMap::new();
            }
        };
        match serde_json::from_str(&json) {
            Ok(keys) => keys,
            Err(error) => {
                report_error!(
                    anyhow::Error::new(error).context("Failed to deserialize custom endpoint keys")
                );
                HashMap::new()
            }
        }
    }

    fn rebuild_custom_endpoints(&mut self) {
        let Some(definitions) = &self.custom_endpoints.definitions else {
            self.custom_endpoints.resolved = self.keys.custom_endpoints.clone();
            return;
        };
        self.custom_endpoints.resolved = definitions
            .definitions()
            .map(|(id, definition)| {
                definition.clone().into_endpoint(
                    self.custom_endpoints
                        .keys
                        .get(id)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect();
    }

    fn write_keys_to_secure_storage(&mut self, ctx: &mut ModelContext<Self>) {
        let json = match serde_json::to_string(&self.keys) {
            Ok(json) => json,
            Err(e) => {
                report_error!(anyhow::Error::new(e).context("Failed to serialize API keys"));
                return;
            }
        };
        self.secure_storage_write_version += 1;
        let write_version = self.secure_storage_write_version;

        // Defer the keychain write so it doesn't block the current event
        // processing. The in-memory state is already updated and events
        // already emitted, so the UI updates immediately while the
        // potentially slow platform secure-storage call runs in a
        // subsequent main-thread callback. Skip stale callbacks so older
        // writes cannot complete after and overwrite a newer payload.
        ctx.spawn(async move { json }, move |me, json, ctx| {
            if write_version != me.secure_storage_write_version {
                return;
            }
            if let Err(e) = ctx.secure_storage().write_value(SECURE_STORAGE_KEY, &json) {
                report_error!(
                    anyhow::Error::new(e).context("Failed to write API keys to secure storage")
                );
            }
        });
    }

    fn load_grok_tokens_from_secure_storage(ctx: &mut ModelContext<Self>) -> Option<GrokTokens> {
        let json = match ctx.secure_storage().read_value(GROK_SECURE_STORAGE_KEY) {
            Ok(json) => json,
            Err(e) => {
                if !matches!(e, secure_storage::Error::NotFound) {
                    report_error!(
                        anyhow::Error::new(e)
                            .context("Failed to read Grok tokens from secure storage")
                    );
                }
                return None;
            }
        };

        match serde_json::from_str(&json) {
            Ok(tokens) => Some(tokens),
            Err(e) => {
                report_error!(anyhow::Error::new(e).context("Failed to deserialize Grok tokens"));
                None
            }
        }
    }

    fn write_grok_tokens_to_secure_storage(&mut self, ctx: &mut ModelContext<Self>) {
        // `Some(json)` writes the tokens; `None` removes the stored entry (the
        // user disconnected). Serialize up front so the deferred callback only
        // touches the keychain.
        let payload = match self.grok_tokens.as_ref().map(serde_json::to_string) {
            Some(Ok(json)) => Some(json),
            Some(Err(e)) => {
                report_error!(anyhow::Error::new(e).context("Failed to serialize Grok tokens"));
                return;
            }
            None => None,
        };
        self.grok_secure_storage_write_version += 1;
        let write_version = self.grok_secure_storage_write_version;

        // Defer the keychain write/remove like `write_keys_to_secure_storage`,
        // skipping stale callbacks so an older write can't clobber a newer one.
        ctx.spawn(async move { payload }, move |me, payload, ctx| {
            if write_version != me.grok_secure_storage_write_version {
                return;
            }
            let result = match payload {
                Some(ref json) => ctx
                    .secure_storage()
                    .write_value(GROK_SECURE_STORAGE_KEY, json),
                None => ctx.secure_storage().remove_value(GROK_SECURE_STORAGE_KEY),
            };
            if let Err(e) = result
                && !matches!(e, secure_storage::Error::NotFound)
            {
                report_error!(
                    anyhow::Error::new(e)
                        .context("Failed to persist Grok tokens to secure storage")
                );
            }
        });
    }
}

impl Entity for ApiKeyManager {
    type Event = ApiKeyManagerEvent;
}

impl SingletonEntity for ApiKeyManager {}

#[cfg(test)]
#[path = "api_keys_tests.rs"]
mod tests;
