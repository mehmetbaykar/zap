//! Bidirectional translation registry for OpenAI tool calling in BYOP mode.
//!
//! Each warp built-in tool (a variant of `api::message::tool_call::Tool`) corresponds to an
//! [`OpenAiTool`] description: function name + JSON Schema + reverse-parsing args + serializing the execution
//! result into a string for the upstream model to read.
//!
//! ## The currently implemented subset (Phase 3a first batch)
//!
//! - `run_shell_command`
//! - `read_files`
//!
//! Later-round expansions: `grep` / `file_glob_v2` / `apply_file_diffs` / `call_mcp_tool`, etc.
//!
//! ## The full loop
//!
//! The model returns `tool_calls` → `from_args` translates them into `tool_call::Tool` → we emit
//! `Message::ToolCall { tool_call_id, tool }` → warp's own `convert_from.rs`
//! automatically translates them into `AIAgentAction` → the executor goes through profile permissions/dialogs → executes → the result
//! is automatically written back into the conversation → triggers the next byop request → our `result_to_json`
//! serializes the result into the content of `role=tool, tool_call_id=...` for the upstream.

pub mod ask;
pub mod coerce;
pub mod documents;
pub mod edit;
pub mod exa;
pub mod files;
pub mod long_shell;
pub mod markers;
pub mod mcp;
pub mod search;
pub mod shell;
pub mod skill;
pub mod suggest;
pub mod todowrite;
pub mod web_runtime;
pub mod webfetch;
pub mod websearch;

use anyhow::Result;
use serde_json::Value;
use warp_multi_agent_api as api;

use crate::ai::agent::AIAgentActionResult;

/// A bidirectional adaptation description for one tool.
///
/// **Naming history**: originally BYOP only connected the OpenAI-compatible protocol, later switching to the genai SDK across 5 adapters
/// (OpenAI / OpenAIResp / Gemini / Anthropic / Ollama). The struct name keeps `OpenAiTool`
/// to preserve git blame, but the JSON Schema it carries is the OpenAPI standard, which each adapter
/// rewrites internally within genai into its respective native format (such as Anthropic input_schema, Gemini function_declarations).
pub struct OpenAiTool {
    /// The function name for the upstream LLM (the model calls it by this name in its response).
    pub name: &'static str,
    /// The description for the LLM.
    pub description: &'static str,
    /// The parameter JSON Schema (OpenAPI standard). Returns a closure to avoid constructing a serde_json::Value in a const.
    pub parameters: fn() -> Value,
    /// Reverse parsing: the args JSON string returned by the upstream model → warp's internal `tool_call::Tool` variant.
    pub from_args: fn(args: &str) -> Result<api::message::tool_call::Tool>,
    /// Converts the `Result` variant in ToolCallResult corresponding to this tool into JSON readable by the upstream model.
    /// Returns `None` when there's no matching variant (letting the caller fall back to generic serialization).
    pub result_to_json: fn(&api::message::tool_call_result::Result) -> Option<Value>,
}

impl OpenAiTool {
    /// Converts to a genai `Tool` (for feeding into `ChatRequest.tools`).
    pub fn to_genai_tool(&self) -> genai::chat::Tool {
        genai::chat::Tool::new(self.name)
            .with_description(self.description)
            .with_schema((self.parameters)())
    }
}

/// Registry: all supported BYOP tools.
pub const REGISTRY: &[&OpenAiTool] = &[
    &shell::RUN_SHELL_COMMAND,
    &files::READ_FILES,
    &search::GREP,
    &search::FILE_GLOB_V2,
    &edit::APPLY_FILE_DIFFS,
    &long_shell::WRITE_TO_LONG_RUNNING_SHELL_COMMAND,
    &long_shell::READ_SHELL_COMMAND_OUTPUT,
    &ask::ASK_USER_QUESTION,
    &skill::READ_SKILL,
    // Local document system (AIDocumentModel)
    &documents::READ_DOCUMENTS,
    &documents::EDIT_DOCUMENTS,
    &documents::CREATE_DOCUMENTS,
    // User suggestion type (local channel + UI)
    &suggest::SUGGEST_NEW_CONVERSATION,
    &suggest::SUGGEST_PROMPT,
    // UI marker (no side effects, signals the frontend)
    &markers::OPEN_CODE_REVIEW,
    &markers::TRANSFER_SHELL_CONTROL,
    // Local todo list (BYOP synthesizes Message::UpdateTodos itself, not going through the protobuf executor)
    &todowrite::TODOWRITE,
    // BYOP-only network tools: not mapped to a protobuf executor variant; chat_stream
    // intercepts them by name before parse_incoming_tool_call and calls web_runtime directly to run the HTTP.
    // gating: when profile.web_search_enabled=false, build_tools_array filters them out.
    &webfetch::WEBFETCH,
    &websearch::WEBSEARCH,
];

/// Reverse-looks up the registry by OpenAI function name.
pub fn lookup(name: &str) -> Option<&'static OpenAiTool> {
    REGISTRY.iter().copied().find(|t| t.name == name)
}

/// Given a ToolCallResult, first finds the corresponding tool in REGISTRY and serializes with its `result_to_json`;
/// if not found, tries the MCP generic serialization; then falls back to a brief description to avoid a panic.
pub fn serialize_result(result: &api::message::ToolCallResult) -> String {
    let inner = match &result.result {
        Some(r) => r,
        None => return r#"{"status":"cancelled"}"#.to_owned(),
    };
    for t in REGISTRY {
        if let Some(json) = (t.result_to_json)(inner) {
            return serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_owned());
        }
    }
    if let Some(json) = mcp::serialize_result(inner) {
        return serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_owned());
    }
    // Fallback: unrecognized variant (tools not yet registered in later rounds also go here).
    r#"{"status":"unsupported_tool_result"}"#.to_owned()
}

/// Serializes the `AIAgentActionResult` from *the current turn's client-side execution* into a JSON string
/// to feed the upstream model (the content of role=tool).
///
/// ## Why not use `AIAgentActionResultType::Display` directly
///
/// The `Display` impl renders structured results (especially `LongRunningCommandSnapshot`) into
/// one-line strings like `"Command 'bun repl' is long-running"`, **completely discarding key fields like block_id
/// (=command_id), grid_contents, is_alt_screen_active**, which means on the next turn the
/// model can't get the command_id and can't continue read/write_to_long_running_*, rendering long-running commands entirely useless.
///
/// ## How it works
///
/// 1. Reuses the existing `TryFrom<AIAgentActionResult>
///    for api::request::input::user_inputs::user_input::Input` in `app/src/ai/agent/api/convert_to.rs` (covering all 25+ ActionResult
///    variants), getting `Input::ToolCallResult { result, .. }`
/// 2. The inner `*Result` types (such as `RunShellCommandResult`) and `api::message::tool_call_result::Result`
///    share the same protobuf message, only differing in the outer enum's namespace, so it can be re-wrapped once into the
///    outer enum, reusing the existing per-tool `result_to_json` in `tools::REGISTRY`
///    (see `shell.rs::result_to_json` flattening `LongRunningCommandSnapshot` into complete JSON
///    including command_id/output/is_alt_screen_active)
/// 3. Unrecognized variants return `None`, and the caller falls back to Display
///
/// ## Maintenance note
///
/// When adding a new BYOP tool, **the enum match here must add a variant in sync**, otherwise that tool's
/// current-turn ActionResult falls back to Display and loses structured fields.
pub fn serialize_action_result(action: &AIAgentActionResult) -> Option<String> {
    let msg_side = action_result_to_msg_result(action)?;
    for t in REGISTRY {
        if let Some(json) = (t.result_to_json)(&msg_side) {
            return Some(serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_owned()));
        }
    }
    if let Some(json) = mcp::serialize_result(&msg_side) {
        return Some(serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_owned()));
    }
    None
}

/// Converts the `AIAgentActionResult` from the current turn's client-side execution into a
/// `api::message::tool_call_result::Result` enum, for BYOP to persist as a task.message.
///
/// Shares `serialize_action_result`'s ReqR → MsgR mapping; the caller then wraps it into
/// `Message::ToolCallResult { result: Some(...), context: None, tool_call_id }`.
pub fn action_result_to_msg_result(
    action: &AIAgentActionResult,
) -> Option<api::message::tool_call_result::Result> {
    use api::message::tool_call_result::Result as MsgR;
    use api::request::input::tool_call_result::Result as ReqR;
    use api::request::input::user_inputs::user_input::Input;

    let input: Input = action.clone().try_into().ok()?;
    let req_input: ReqR = match input {
        Input::ToolCallResult(tcr) => tcr.result?,
        _ => return None,
    };
    let msg_side = match req_input {
        ReqR::RunShellCommand(r) => MsgR::RunShellCommand(r),
        ReqR::WriteToLongRunningShellCommand(r) => MsgR::WriteToLongRunningShellCommand(r),
        ReqR::ReadShellCommandOutput(r) => MsgR::ReadShellCommandOutput(r),
        ReqR::ReadFiles(r) => MsgR::ReadFiles(r),
        ReqR::Grep(r) => MsgR::Grep(r),
        ReqR::FileGlobV2(r) => MsgR::FileGlobV2(r),
        ReqR::ApplyFileDiffs(r) => MsgR::ApplyFileDiffs(r),
        ReqR::CallMcpTool(r) => MsgR::CallMcpTool(r),
        ReqR::ReadMcpResource(r) => MsgR::ReadMcpResource(r),
        ReqR::AskUserQuestion(r) => MsgR::AskUserQuestion(r),
        ReqR::ReadSkill(r) => MsgR::ReadSkill(r),
        ReqR::ReadDocuments(r) => MsgR::ReadDocuments(r),
        ReqR::EditDocuments(r) => MsgR::EditDocuments(r),
        ReqR::CreateDocuments(r) => MsgR::CreateDocuments(r),
        ReqR::SuggestNewConversation(r) => MsgR::SuggestNewConversation(r),
        ReqR::SuggestPrompt(r) => MsgR::SuggestPrompt(r),
        ReqR::OpenCodeReview(r) => MsgR::OpenCodeReview(r),
        ReqR::TransferShellCommandControlToUser(r) => MsgR::TransferShellCommandControlToUser(r),
        _ => return None,
    };
    Some(msg_side)
}
