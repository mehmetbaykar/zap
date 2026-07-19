//! Child-agent orchestration tool: `start_agent`.
//!
//! Lets the BYOP lead agent spawn a local child agent (embedded native agent, or a
//! third-party CLI harness like Claude Code / OpenCode / Codex running in a hidden
//! pane). Maps onto the protobuf `StartAgentV2` tool so the whole existing executor
//! chain is reused: `convert_from.rs` → `AIAgentActionType::StartAgent` →
//! `StartAgentExecutor` (profile `run_agents` permission + confirmation UI) →
//! `terminal_pane` child-conversation launch → `StartAgentResult` written back.
//!
//! Gating (see `chat_stream::build_tools_array` / `available_tool_names`):
//! - `RequestParams.run_agents_enabled` — the active profile's `run_agents`
//!   permission is `NeverAllow` → tool not exposed.
//! - `RequestParams.parent_agent_id.is_some()` — child agents cannot spawn
//!   grandchildren (single-level recursion guard, mirroring upstream).
//! - Plan Mode blocks it (side-effectful).
//! - `chat_stream` additionally caps spawns per assistant turn
//!   (`MAX_START_AGENT_CALLS_PER_TURN`).

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use warp_multi_agent_api as api;

use super::OpenAiTool;

pub const TOOL_NAME: &str = "start_agent";

/// Upper bound on `start_agent` calls honored in a single assistant turn.
/// Calls beyond this get a synthetic error result instead of spawning.
pub const MAX_START_AGENT_CALLS_PER_TURN: usize = 4;

/// Harness values the local child launcher accepts
/// (`Harness::parse_local_child_harness`): "claude" | "opencode" | "codex".
/// Empty/omitted selects the embedded native child agent.
#[derive(Debug, Deserialize)]
struct Args {
    name: String,
    prompt: String,
    #[serde(default)]
    harness: String,
}

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Short display name for the child agent (e.g. \"Test Fixer\")."
            },
            "prompt": {
                "type": "string",
                "description": "The complete, self-contained task prompt for the child agent. The child has no access to this conversation's history, so include all necessary context."
            },
            "harness": {
                "type": "string",
                "enum": ["claude", "opencode", "codex"],
                "description": "Optional: run the child inside a third-party CLI harness installed on this machine. Omit to use the built-in native agent."
            }
        },
        "required": ["name", "prompt"],
        "additionalProperties": false
    })
}

fn from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    let parsed: Args = serde_json::from_str(args)?;
    let harness = parsed.harness.trim();
    let local = api::start_agent_v2::execution_mode::Local {
        harness: (!harness.is_empty()).then(|| api::start_agent_v2::execution_mode::Harness {
            r#type: harness.to_string(),
        }),
    };
    Ok(api::message::tool_call::Tool::StartAgentV2(
        api::StartAgentV2 {
            name: parsed.name,
            prompt: parsed.prompt,
            // Omitted = subscribe to all lifecycle event types (proto default).
            lifecycle_subscription: None,
            execution_mode: Some(api::start_agent_v2::ExecutionMode {
                mode: Some(api::start_agent_v2::execution_mode::Mode::Local(local)),
            }),
        },
    ))
}

fn result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    let value = match result {
        R::StartAgent(r) => match &r.result {
            Some(api::start_agent_result::Result::Success(s)) => {
                json!({ "status": "started", "agent_id": s.agent_id })
            }
            Some(api::start_agent_result::Result::Error(e)) => {
                json!({ "status": "error", "error": e.error })
            }
            None => json!({ "status": "cancelled" }),
        },
        R::StartAgentV2(r) => match &r.result {
            Some(api::start_agent_v2_result::Result::Success(s)) => {
                json!({ "status": "started", "agent_id": s.agent_id })
            }
            Some(api::start_agent_v2_result::Result::Error(e)) => {
                json!({ "status": "error", "error": e.error })
            }
            None => json!({ "status": "cancelled" }),
        },
        _ => return None,
    };
    Some(value)
}

pub static START_AGENT: OpenAiTool = OpenAiTool {
    name: TOOL_NAME,
    description: include_str!("../prompts/tool_descriptions/start_agent.md"),
    parameters,
    from_args,
    result_to_json,
};

#[cfg(test)]
#[path = "start_agent_tests.rs"]
mod tests;
