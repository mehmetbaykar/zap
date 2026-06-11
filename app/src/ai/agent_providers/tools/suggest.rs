//! User-prompting tools: `suggest_new_conversation` / `suggest_prompt`.
//!
//! Both tools are **pure local channel signals** + a UI dialog — the model proactively suggests an action,
//! the user accepts/rejects it in the UI, and the executor writes back the result after the user decides. They don't depend on any server.

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use warp_multi_agent_api as api;

use super::OpenAiTool;

// ---------------------------------------------------------------------------
// suggest_new_conversation
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct NewConvArgs {
    /// The id of the current assistant message (if the model doesn't know it, it can pass an empty string and the controller will fill it in).
    #[serde(default)]
    message_id: String,
}

fn new_conv_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "message_id": {
                "type": "string",
                "description": "Optional: which assistant message to branch the new conversation from (leave empty to use the current message)."
            }
        },
        "additionalProperties": false
    })
}

fn new_conv_from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    let parsed: NewConvArgs = if args.trim().is_empty() {
        NewConvArgs {
            message_id: String::new(),
        }
    } else {
        serde_json::from_str(args)?
    };
    Ok(api::message::tool_call::Tool::SuggestNewConversation(
        api::message::tool_call::SuggestNewConversation {
            message_id: parsed.message_id,
        },
    ))
}

fn new_conv_result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    use api::suggest_new_conversation_result::Result as SR;
    let r = match result {
        R::SuggestNewConversation(r) => r,
        _ => return None,
    };
    let value = match &r.result {
        Some(SR::Accepted(a)) => json!({ "status": "accepted", "message_id": a.message_id }),
        Some(SR::Rejected(_)) => json!({ "status": "rejected" }),
        None => json!({ "status": "cancelled" }),
    };
    Some(value)
}

pub static SUGGEST_NEW_CONVERSATION: OpenAiTool = OpenAiTool {
    name: "suggest_new_conversation",
    description: "Suggests that the user branch a new conversation from the current message. \
                  Applicable when the current conversation context is already long and is about to switch topics, or when the current task has ended and \
                  the next task is unrelated to it. The UI pops up a confirmation box, and it only branches once the user accepts. \
                  **Don't abuse it** — only call it when the benefit of switching context is clear.",
    parameters: new_conv_parameters,
    from_args: new_conv_from_args,
    result_to_json: new_conv_result_to_json,
};

// ---------------------------------------------------------------------------
// suggest_prompt
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PromptArgs {
    /// The prompt text actually sent to the agent.
    prompt: String,
    /// Optional: a short label shown in the UI (used for chip display if the prompt is too long).
    #[serde(default)]
    label: String,
}

fn prompt_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "prompt": {
                "type": "string",
                "description": "The next prompt to suggest to the user (sent to the agent when the user clicks it)."
            },
            "label": {
                "type": "string",
                "description": "Optional: a short label shown on the chip (recommended when the prompt is long)."
            }
        },
        "required": ["prompt"],
        "additionalProperties": false
    })
}

fn prompt_from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    use api::message::tool_call::suggest_prompt::{DisplayMode, PromptChip};
    let parsed: PromptArgs = serde_json::from_str(args)?;
    let chip = PromptChip {
        prompt: parsed.prompt,
        label: parsed.label,
    };
    Ok(api::message::tool_call::Tool::SuggestPrompt(
        api::message::tool_call::SuggestPrompt {
            display_mode: Some(DisplayMode::PromptChip(chip)),
            is_trigger_irrelevant: false,
        },
    ))
}

fn prompt_result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    use api::suggest_prompt_result::Result as SR;
    let r = match result {
        R::SuggestPrompt(r) => r,
        _ => return None,
    };
    let value = match &r.result {
        Some(SR::Accepted(_)) => json!({ "status": "accepted" }),
        Some(SR::Rejected(_)) => json!({ "status": "rejected" }),
        None => json!({ "status": "cancelled" }),
    };
    Some(value)
}

pub static SUGGEST_PROMPT: OpenAiTool = OpenAiTool {
    name: "suggest_prompt",
    description: "Proposes the next prompt to the user at the end of the answer (shown as a chip). \
                  Applicable when the task naturally extends into an obvious follow-up (suggest running lint after tests pass; suggest adding unit tests after reading code, etc.). \
                  Avoid giving repetitive or obvious suggestions.",
    parameters: prompt_parameters,
    from_args: prompt_from_args,
    result_to_json: prompt_result_to_json,
};
