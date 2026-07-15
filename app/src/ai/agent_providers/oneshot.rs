//! BYOP one-shot non-streaming completion adaptation layer.
//!
//! Used for the "proactive AI" sub-chains (prompt suggestions / NLD predict / relevant files /
//! session title generation, etc.): they need to send one short request to get a piece of text, **without tool calling,
//! without streaming, without persisting to task.messages**.
//!
//! Differences from `chat_stream::generate_byop_output` (the main conversation stream):
//! - Here it uses `Client::exec_chat` (non-streaming), getting `ChatResponse::first_text()` in one shot.
//! - It doesn't touch `RequestParams` / `ResponseEvent` / `task_store`; pure string in, string out.
//! - reasoning is disabled by default (proactive AI should not trigger chain-of-thought — wastes tokens + slow),
//!   it's only injected per the capability gate when `OneshotOptions.allow_reasoning = true`.
//!
//! Model selection is decided by the caller: `resolve_active_ai_oneshot()` decodes `active_ai_model`
//! (profile, falling back to base_model) into a BYOP `OneshotConfig`;
//! on decode failure (BYOP not configured / model not in the BYOP encoding space) → returns `None`,
//! and the caller silently no-ops.

use anyhow::Context as _;
use futures::StreamExt;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent};
use warpui::{AppContext, EntityId, SingletonEntity as _};

use super::chat_stream;
use crate::ai::agent::redaction;
use crate::ai::llms::LLMPreferences;
use crate::settings::{AgentProviderApiType, ReasoningEffortSetting};
use crate::terminal::safe_mode_settings::get_secret_obfuscation_mode;

/// The provider/model information needed for a BYOP one-shot request.
#[derive(Debug, Clone)]
pub struct OneshotConfig {
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    pub api_type: AgentProviderApiType,
    pub reasoning_effort: ReasoningEffortSetting,
    /// Safe Mode snapshot taken at resolve time. One-shot prompts embed terminal
    /// content (block outputs, history, diffs), and BYOP has no backend-side
    /// redaction, so detected secrets must be masked here before send — the same
    /// contract `RequestParams::new` enforces for the main conversation stream.
    pub should_redact_secrets: bool,
}

/// Optional parameters for a one-shot call.
#[derive(Debug, Clone, Default)]
pub struct OneshotOptions {
    /// Upper limit for truncating the user message (by char, to protect CJK). `None` = default 8000.
    pub max_chars: Option<usize>,
    /// Temperature (genai `ChatOptions::temperature`); `None` = provider default.
    pub temperature: Option<f32>,
    /// Whether to require JSON output (OpenAI-compatible providers use response_format).
    /// Note: adapters that don't support it ignore this parameter, so the system prompt needs to require JSON itself.
    pub response_format_json: bool,
    /// Whether reasoning is allowed to trigger. Defaults to `false` (proactive AI is all low-latency lightweight calls).
    pub allow_reasoning: bool,
}

const DEFAULT_MAX_CHARS: usize = 8000;

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    s.chars().take(max).collect()
}

fn build_oneshot_request(
    cfg: &OneshotConfig,
    system: &str,
    user: &str,
    opts: &OneshotOptions,
) -> (ChatRequest, ChatOptions) {
    let mut chat_opts = ChatOptions::default()
        .with_capture_content(true)
        .with_capture_usage(true);
    if let Some(t) = opts.temperature {
        chat_opts = chat_opts.with_temperature(t.into());
    }
    if opts.response_format_json {
        chat_opts = chat_opts.with_response_format(genai::chat::ChatResponseFormat::JsonMode);
    }
    if opts.allow_reasoning {
        if let Some(effort) = cfg.reasoning_effort.to_genai() {
            if super::reasoning::model_supports_reasoning(cfg.api_type, &cfg.model_id) {
                chat_opts = chat_opts.with_reasoning_effort(effort);
            }
        }
    }

    let max_chars = opts.max_chars.unwrap_or(DEFAULT_MAX_CHARS);
    let mut user_truncated = truncate_chars(user, max_chars);
    let mut system = system.to_owned();
    if cfg.should_redact_secrets {
        redaction::redact_secrets(&mut system);
        redaction::redact_secrets(&mut user_truncated);
    }

    let chat_req =
        ChatRequest::from_messages(vec![ChatMessage::user(user_truncated)]).with_system(system);

    (chat_req, chat_opts)
}

/// Flattens a genai error into a bounded message: provider HTTP errors embed
/// the response body in their Display, which can echo request content into
/// callers' default-level logs (title generation, active AI, code review).
fn bounded_oneshot_err(e: genai::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "{}",
        super::openai_compatible::bounded_provider_body(e.to_string())
    )
}

/// Sends one BYOP non-streaming chat completion, returning the plain text of the model's reply.
///
/// Error swallowing is decided by the caller — here we only propagate `anyhow::Error` and don't log.
pub async fn byop_oneshot_completion(
    cfg: &OneshotConfig,
    system: &str,
    user: &str,
    opts: &OneshotOptions,
) -> anyhow::Result<String> {
    let client = chat_stream::build_client(cfg.api_type, &cfg.base_url, cfg.api_key.clone());
    let (chat_req, chat_opts) = build_oneshot_request(cfg, system, user, opts);

    let resp = client
        .exec_chat(&cfg.model_id, chat_req, Some(&chat_opts))
        .await
        .map_err(bounded_oneshot_err)
        .with_context(|| format!("byop oneshot exec_chat failed (model={})", cfg.model_id))?;

    Ok(resp.first_text().unwrap_or("").to_owned())
}

/// Sends one BYOP streaming chat completion, aggregating all text chunks before returning.
///
/// For use with OpenAI Responses-compatible proxies that only accept `stream=true`. The caller still gets the complete
/// string, so it can continue to reuse the one-shot title-cleaning / JSON-parsing logic.
pub async fn byop_oneshot_streaming_completion(
    cfg: &OneshotConfig,
    system: &str,
    user: &str,
    opts: &OneshotOptions,
) -> anyhow::Result<String> {
    let client = chat_stream::build_client(cfg.api_type, &cfg.base_url, cfg.api_key.clone());
    let (chat_req, chat_opts) = build_oneshot_request(cfg, system, user, opts);
    let mut resp = client
        .exec_chat_stream(&cfg.model_id, chat_req, Some(&chat_opts))
        .await
        .map_err(bounded_oneshot_err)
        .with_context(|| {
            format!(
                "byop oneshot exec_chat_stream failed (model={})",
                cfg.model_id
            )
        })?
        .stream;

    let mut text = String::new();
    while let Some(event) = resp.next().await {
        match event.map_err(bounded_oneshot_err).with_context(|| {
            format!(
                "byop oneshot exec_chat_stream event failed (model={})",
                cfg.model_id
            )
        })? {
            ChatStreamEvent::Chunk(chunk) => {
                text.push_str(&chunk.content);
            }
            ChatStreamEvent::Start
            | ChatStreamEvent::ReasoningChunk(_)
            | ChatStreamEvent::ThoughtSignatureChunk(_)
            | ChatStreamEvent::ToolCallChunk(_)
            | ChatStreamEvent::End(_) => {}
        }
    }

    Ok(text)
}

/// Resolves the current active profile's `active_ai_model` (falling back to `base_model`);
/// if it decodes to a valid BYOP encoding → returns `OneshotConfig`, otherwise `None` (the caller silently no-ops).
pub fn resolve_active_ai_oneshot(
    app: &AppContext,
    terminal_view_id: Option<EntityId>,
) -> Option<OneshotConfig> {
    let llm_prefs = LLMPreferences::as_ref(app);
    let id = llm_prefs
        .get_active_ai_model(app, terminal_view_id)
        .id
        .clone();
    let (provider, api_key, model_id) = super::lookup_byop(app, &id)?;
    let reasoning_effort =
        llm_prefs.get_reasoning_effort(terminal_view_id, provider.api_type, &model_id);
    Some(OneshotConfig {
        base_url: provider.base_url,
        api_key,
        model_id,
        api_type: provider.api_type,
        reasoning_effort,
        should_redact_secrets: get_secret_obfuscation_mode(app).should_redact_secret(),
    })
}

/// Resolves the current active profile's `next_command_model` (falling back to `base_model`);
/// if it decodes to a valid BYOP encoding → returns `OneshotConfig`, otherwise `None`.
pub fn resolve_next_command_oneshot(
    app: &AppContext,
    terminal_view_id: Option<EntityId>,
) -> Option<OneshotConfig> {
    let llm_prefs = LLMPreferences::as_ref(app);
    let id = llm_prefs
        .get_active_next_command_model(app, terminal_view_id)
        .id
        .clone();
    let (provider, api_key, model_id) = super::lookup_byop(app, &id)?;
    let reasoning_effort =
        llm_prefs.get_reasoning_effort(terminal_view_id, provider.api_type, &model_id);
    Some(OneshotConfig {
        base_url: provider.base_url,
        api_key,
        model_id,
        api_type: provider.api_type,
        reasoning_effort,
        should_redact_secrets: get_secret_obfuscation_mode(app).should_redact_secret(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::model::secrets::set_user_and_enterprise_secret_regexes;

    fn test_cfg(should_redact_secrets: bool) -> OneshotConfig {
        OneshotConfig {
            base_url: "http://localhost".to_string(),
            api_key: "key".to_string(),
            model_id: "model".to_string(),
            api_type: AgentProviderApiType::OpenAi,
            reasoning_effort: ReasoningEffortSetting::default(),
            should_redact_secrets,
        }
    }

    // Guards the one-shot Safe Mode chokepoint: every proactive-AI request
    // (next command, input suggestions, titles, code-review summaries) funnels
    // through `build_oneshot_request`, and BYOP has no backend to redact
    // server-side, so detected secrets must be masked here before send.
    #[test]
    fn build_oneshot_request_redacts_secrets_when_safe_mode_on() {
        let re = regex::Regex::new(r"SECRET-\d+").expect("valid regex");
        let none: [&regex::Regex; 0] = [];
        set_user_and_enterprise_secret_regexes([&re], none);

        let opts = OneshotOptions::default();
        let (req, _) = build_oneshot_request(
            &test_cfg(true),
            "system with SECRET-11111",
            "user with SECRET-22222",
            &opts,
        );
        let system = req.system.clone().unwrap_or_default();
        assert!(!system.contains("SECRET-11111"), "system: {system}");
        assert!(system.contains('*'), "system should be masked: {system}");
        let user = req.messages[0].content.first_text().unwrap_or_default();
        assert!(!user.contains("SECRET-22222"), "user: {user}");

        let (req, _) = build_oneshot_request(
            &test_cfg(false),
            "system with SECRET-11111",
            "user with SECRET-22222",
            &opts,
        );
        assert!(
            req.system
                .clone()
                .unwrap_or_default()
                .contains("SECRET-11111"),
            "safe mode off must not alter the prompt"
        );
    }
}
