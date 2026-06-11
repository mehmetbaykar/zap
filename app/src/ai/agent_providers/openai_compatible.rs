//! A minimal subset of an OpenAI-compatible client: currently only used to fetch the `/models` list.
//!
//! When the second phase adds multi-agent calls, this will be expanded into a complete
//! Chat Completions + tool-calling stream.

use serde::Deserialize;

use http_client::Client;

/// A single model entry returned by the `/models` endpoint.
///
/// We only care about `id` (used by the Agent as the model name). Other fields (`object`/`created`/`owned_by`)
/// vary significantly across providers and are all ignored here.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct OpenAiCompatibleModel {
    pub id: String,
    /// The owner inferred from `owned_by`, mainly for UI display; may be empty.
    #[serde(default)]
    pub owned_by: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<OpenAiCompatibleModel>,
}

/// Errors that may occur during fetch.
#[derive(Debug, thiserror::Error)]
pub enum OpenAiCompatibleError {
    #[error("invalid base URL: {0}")]
    InvalidBaseUrl(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("HTTP status code {status}: {body}")]
    Status { status: u16, body: String },

    #[error("response parsing failed: {0}")]
    Decode(String),

    #[error("network/streaming request failed: {0}")]
    Stream(String),

    #[error("call failed: {0}")]
    Other(String),
}

/// Normalizes the user-entered base_url into an absolute URL form,
/// tolerating a trailing `/`, a missing `/v1`, `/openai/v1`, etc.
pub(crate) fn normalize_base_url(input: &str) -> Result<String, OpenAiCompatibleError> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(OpenAiCompatibleError::InvalidBaseUrl(
            "base URL cannot be empty".to_string(),
        ));
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(OpenAiCompatibleError::InvalidBaseUrl(format!(
            "base URL must start with http:// or https://: {trimmed}"
        )));
    }
    Ok(trimmed.to_string())
}

/// Calls `${base_url}/models`, returning the list of model IDs (deduplicated + alphabetically sorted).
///
/// Authentication: if `api_key` is non-empty, it's attached as `Authorization: Bearer ...`.
/// Some local services (such as Ollama) allow no authentication, so the header isn't sent when the key is empty.
pub async fn fetch_openai_compatible_models(
    client: Client,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<OpenAiCompatibleModel>, OpenAiCompatibleError> {
    let base = normalize_base_url(base_url)?;
    let url = format!("{base}/models");

    let mut req = client.get(&url);
    if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        req = req.bearer_auth(key);
    }

    let response = req.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(OpenAiCompatibleError::Status {
            status: status.as_u16(),
            body,
        });
    }

    let parsed: ModelsResponse = response
        .json()
        .await
        .map_err(|e| OpenAiCompatibleError::Decode(e.to_string()))?;

    let mut models = parsed.data;
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}
