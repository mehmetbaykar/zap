//! models.dev data source integration.
//!
//! When the user opens the Providers settings page, the background asynchronously fetches `https://models.dev/api.json`,
//! caching it to `${cache_dir}/models-dev.json`. The next startup reads the cache directly,
//! and on a cache hit within the TTL (default 24h) no request is sent; it fetches again when expired/missing.
//!
//! The data structure aligns with opencode's `provider/models.ts`: the top level is
//! `{ <provider_id>: Provider }`, where Provider contains `models: { <model_id>: Model }`.
//! We only care about the few fields the UI "quick select" needs:
//! - provider: id / name / api / env (hints which env var is needed)
//! - model:    id / name / limit.context / limit.output / reasoning / tool_call
//!
//! Unlisted fields are all tolerated via `serde(default)` + `#[allow(dead_code)]`.
//!
//! Design tradeoff: **synchronous cache read, asynchronous network fetch**. The read side is for the UI and must be fast;
//! the fetch side is spawned in the background, doesn't pop an error on failure (only logs), and gives empty data if the cache can't be read, with the UI showing
//! "models.dev not fetched yet, please check your network".

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::sync::RwLock;
use std::time::{Duration, SystemTime};

use http_client::Client;
use serde::{Deserialize, Serialize};

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CACHE_FILENAME: &str = "models-dev.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// `models.dev` top-level data — provider_id → Provider.
pub type Catalog = BTreeMap<String, Provider>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Provider {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Upstream API base URL, e.g. `https://api.deepseek.com/v1`.
    #[serde(default)]
    pub api: Option<String>,
    /// The environment variable names this provider typically needs, e.g. `["DEEPSEEK_API_KEY"]`.
    #[serde(default)]
    pub env: Vec<String>,
    /// Available models, keyed by model id.
    #[serde(default)]
    pub models: BTreeMap<String, Model>,
    /// Documentation URL (some providers have one).
    #[serde(default)]
    pub doc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Model {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default = "default_true")]
    pub tool_call: bool,
    /// Whether file attachments are supported (the attachment field, complementary to modalities:
    /// modalities describe native multimodality; attachment covers PDF / general file attachment protocols).
    #[serde(default)]
    pub attachment: bool,
    /// Input / output modalities, typical values: `text` / `image` / `audio` / `video` / `pdf`.
    #[serde(default)]
    pub modalities: ModelModalities,
    /// Context window upper limit.
    #[serde(default)]
    pub limit: ModelLimit,
    /// "alpha" / "beta" / "deprecated" label.
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

impl ModelModalities {
    pub fn supports_input(&self, modality: &str) -> bool {
        self.input.iter().any(|m| m.eq_ignore_ascii_case(modality))
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelLimit {
    #[serde(default)]
    pub context: u32,
    #[serde(default)]
    pub output: u32,
}

// ── in-process singleton cache ──────────────────────────────────────────────

#[derive(Debug, Default)]
struct State {
    /// The loaded catalog. `None` means it was never loaded successfully.
    catalog: Option<Catalog>,
    /// The cache's last modified time (used to determine whether it's expired).
    loaded_at: Option<SystemTime>,
}

fn state() -> &'static RwLock<State> {
    static S: OnceLock<RwLock<State>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(State::default()))
}

fn cache_path() -> PathBuf {
    let mut p = warp_core::paths::cache_dir();
    p.push(CACHE_FILENAME);
    p
}

/// Reads a copy of the loaded catalog (no lock waiting — clones directly).
/// Returns `None` if there's no data, and the UI should show a "fetching" / retry button.
pub fn cached() -> Option<Catalog> {
    state().read().ok().and_then(|s| s.catalog.clone())
}

/// A capability snapshot extracted for a model from models.dev, used for BYOP UI / chat_stream attachment-type decisions.
#[derive(Debug, Clone, Default)]
pub struct ModelCaps {
    pub vision: bool,
    pub pdf: bool,
    pub audio: bool,
    pub attachment: bool,
}

impl ModelCaps {
    pub fn from_model(m: &Model) -> Self {
        Self {
            vision: m.modalities.supports_input("image"),
            pdf: m.modalities.supports_input("pdf") || m.attachment,
            audio: m.modalities.supports_input("audio"),
            attachment: m.attachment,
        }
    }
}

/// Looks up by model_id in the loaded catalog, returning the capabilities this model declares on models.dev.
///
/// It first uses `provider_id` to exactly match the catalog provider key; on a miss it falls back to "scanning the whole catalog
/// for the first model.id match". This both allows an exact match (when the user-entered provider.id matches models.dev),
/// and handles user-custom provider ids (e.g. aggregator providers like "openrouter" or "siliconflow"
/// that forward upstream models, whose id differs from the upstream provider on models.dev).
pub fn lookup_caps(provider_id: &str, model_id: &str) -> Option<ModelCaps> {
    let s = state().read().ok()?;
    let catalog = s.catalog.as_ref()?;
    if let Some(p) = catalog.get(provider_id) {
        if let Some(m) = p.models.get(model_id) {
            return Some(ModelCaps::from_model(m));
        }
    }
    for p in catalog.values() {
        if let Some(m) = p.models.get(model_id) {
            return Some(ModelCaps::from_model(m));
        }
    }
    None
}

/// Reads the disk cache into memory (synchronous, non-blocking; called only at process startup or the first time the UI needs it).
/// If the disk cache doesn't exist or fails to parse, returns false, and the caller should trigger a network fetch.
pub fn load_from_disk() -> bool {
    let path = cache_path();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let mtime = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok());
    match serde_json::from_slice::<Catalog>(&bytes) {
        Ok(catalog) => {
            if let Ok(mut s) = state().write() {
                s.catalog = Some(catalog);
                s.loaded_at = mtime;
            }
            true
        }
        Err(e) => {
            log::warn!("[models.dev] failed to parse disk cache ({path:?}): {e}");
            false
        }
    }
}

/// Whether the cache is stale — absent or past the TTL.
pub fn is_stale() -> bool {
    let s = match state().read() {
        Ok(s) => s,
        Err(_) => return true,
    };
    match s.loaded_at {
        Some(t) => SystemTime::now()
            .duration_since(t)
            .map(|d| d > CACHE_TTL)
            .unwrap_or(true),
        None => true,
    }
}

/// Asynchronously fetches models.dev and writes to both the disk cache and the in-memory cache.
/// On failure it only logs and doesn't propagate upward (the UI caller decides what to show based on whether `cached()` is `Some`).
pub async fn fetch_and_cache(client: Client) -> Result<(), String> {
    let resp = client
        .get(MODELS_DEV_URL)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("failed to read response body: {e}"))?;

    let catalog: Catalog =
        serde_json::from_slice(&bytes).map_err(|e| format!("JSON parsing failed: {e}"))?;

    // Write to disk — failure isn't fatal, only logged.
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, &bytes) {
        log::warn!("[models.dev] failed to write disk cache ({path:?}): {e}");
    }

    if let Ok(mut s) = state().write() {
        s.catalog = Some(catalog);
        s.loaded_at = Some(SystemTime::now());
    }
    Ok(())
}

// ── chip row collapse/expand state (process-level, to avoid losing it on widget rebuild) ─────────────────

static CHIPS_EXPANDED: AtomicBool = AtomicBool::new(false);

pub fn chips_expanded() -> bool {
    CHIPS_EXPANDED.load(Ordering::Relaxed)
}

pub fn toggle_chips_expanded() {
    CHIPS_EXPANDED.fetch_xor(true, Ordering::Relaxed);
}

// ── search filter for the quick-add chip row ──────────────────────────────────────────────

fn search_state() -> &'static RwLock<String> {
    static S: OnceLock<RwLock<String>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(String::new()))
}

pub fn search_query() -> String {
    search_state()
        .read()
        .ok()
        .map(|s| s.clone())
        .unwrap_or_default()
}

pub fn set_search_query(q: String) {
    if let Ok(mut s) = search_state().write() {
        *s = q;
    }
}

/// Filters the catalog by the current search query, case-insensitively substring-matching provider.name and provider.id.
/// An empty query returns all entries in order. Returns an owned Vec so the UI side can take/iter.
pub fn filter_catalog(catalog: &Catalog, query: &str) -> Vec<(String, Provider)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return catalog
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
    }
    catalog
        .iter()
        .filter(|(id, p)| id.to_lowercase().contains(&q) || p.name.to_lowercase().contains(&q))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Converts a models.dev Model into the AgentProviderModel used by local settings.
///
/// By default it writes the catalog-inferred image/pdf/audio into the fields (so on the user's first sync / quick-add
/// they directly see the model capabilities synced into the toml, without needing to expand the detail to see them).
/// On a later sync the caller only fills new values into None slots, treating Some(_) as a user explicit override to skip.
pub fn into_agent_provider_model(model: &Model) -> crate::settings::AgentProviderModel {
    let caps = ModelCaps::from_model(model);
    crate::settings::AgentProviderModel {
        name: if model.name.is_empty() {
            model.id.clone()
        } else {
            model.name.clone()
        },
        id: model.id.clone(),
        context_window: model.limit.context,
        max_output_tokens: model.limit.output,
        reasoning: model.reasoning,
        tool_call: model.tool_call,
        image: Some(caps.vision),
        pdf: Some(caps.pdf),
        audio: Some(caps.audio),
    }
}
