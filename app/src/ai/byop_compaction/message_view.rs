//! Adapts the warp `api::Message` sequence to the [`MessageRef`] trait for [`super::algorithm`] to operate on.
//!
//! ## Semantic mapping to opencode `MessageV2.WithParts`
//!
//! opencode: a single user/assistant message contains multiple parts (text/tool/file/...);
//! warp: a single protobuf `api::Message` is fine-grained (UserQuery / AgentReasoning / AgentOutput / ToolCall / ToolCallResult are each independent).
//!
//! This projection treats each warp `api::Message` as a `MessageRef` **one-to-one**,
//! with turn detection still cutting on user message boundaries — a user message followed by consecutive non-user messages is one turn.
//! This does not affect the correctness of the [`super::algorithm::turns`] / [`super::algorithm::select`] algorithms.
//!
//! Prune decisions target `Role::Tool` (ToolCallResult) — each ToolCallResult is itself a candidate.
//! The caller must index `tool_call_id → tool_name` for all ToolCalls in the conversation into [`ToolNameLookup`] beforehand.

use std::collections::HashMap;

use warp_multi_agent_api as api;

use super::algorithm::{MessageRef, Role, ToolOutputRef};
use super::state::CompactionState;

/// `tool_call_id → tool_name` index, used during projection to:
/// 1. Annotate each ToolCallResult with tool_name (for the PRUNE_PROTECTED_TOOLS check)
/// 2. Let prune decisions skip protected tools (such as `skill`)
pub type ToolNameLookup = HashMap<String, String>;

/// Given a set of tasks, extracts the `(tool_call_id, tool_name)` pairs for all ToolCalls.
pub fn build_tool_name_lookup<'a, I>(messages: I) -> ToolNameLookup
where
    I: IntoIterator<Item = &'a api::Message>,
{
    let mut out = ToolNameLookup::new();
    for msg in messages {
        if let Some(api::message::Message::ToolCall(tc)) = &msg.message {
            // Use the enum variant name of the protobuf tool_call.tool directly
            let name = tool_name_for(tc).unwrap_or_default();
            out.insert(tc.tool_call_id.clone(), name);
        }
    }
    out
}

/// Gets the "tool name" from a protobuf ToolCall.
///
/// This projection only needs to recognize tools in [`PRUNE_PROTECTED_TOOLS`](`super::consts::PRUNE_PROTECTED_TOOLS`)
/// (currently only "skill", corresponding to warp's `Tool::ReadSkill`); other tools return an empty string — in prune decisions
/// the empty string matches no protected entry, which is the correct behavior (allowed to be pruned).
fn tool_name_for(tc: &api::message::ToolCall) -> Option<String> {
    use api::message::tool_call::Tool;
    let t = tc.tool.as_ref()?;
    let s = match t {
        Tool::ReadSkill(_) => "skill",
        _ => "",
    };
    Some(s.to_string())
}

/// A view of a single `api::Message`.
#[derive(Clone, Copy)]
pub struct WarpMessageView<'a> {
    pub msg: &'a api::Message,
    pub state: &'a CompactionState,
    pub tool_names: &'a ToolNameLookup,
}

/// Estimates the token usage of a single message — sum of visible text character count / 4.
fn estimate_message(msg: &api::Message) -> usize {
    use super::token::estimate;
    use api::message::Message as M;
    let chars = msg
        .message
        .as_ref()
        .map(|inner| match inner {
            M::UserQuery(u) => u.query.chars().count(),
            M::AgentOutput(a) => a.text.chars().count(),
            M::AgentReasoning(r) => r.reasoning.chars().count(),
            M::ToolCall(_) => msg.server_message_data.chars().count().max(64),
            M::ToolCallResult(tcr) => {
                // Prefer the estimate from the result oneof; fall back to server_message_data.
                // Simplification: both are computed by character count, with result.estimate using the Debug repr.
                let from_oneof = tcr
                    .result
                    .as_ref()
                    .map(|r| format!("{r:?}").chars().count())
                    .unwrap_or(0);
                from_oneof
                    .max(msg.server_message_data.chars().count())
                    .max(32)
            }
            _ => 0,
        })
        .unwrap_or(0);
    // Same algorithm as opencode: chars / 4 round.
    estimate(&" ".repeat(chars))
}

impl<'a> MessageRef for WarpMessageView<'a> {
    type Id = String;
    type CallId = String;

    fn id(&self) -> String {
        self.msg.id.clone()
    }

    fn role(&self) -> Role {
        use api::message::Message as M;
        match &self.msg.message {
            Some(M::UserQuery(_)) => Role::User,
            Some(M::ToolCallResult(_)) => Role::Tool,
            // AgentOutput / AgentReasoning / ToolCall / others → Assistant
            _ => Role::Assistant,
        }
    }

    fn is_compaction_marker(&self) -> bool {
        // Only counts for a user message that carries a compaction_trigger marker
        if self.role() != Role::User {
            return false;
        }
        self.state
            .marker(&self.msg.id)
            .map(|m| m.compaction_trigger.is_some())
            .unwrap_or(false)
    }

    fn is_summary(&self) -> bool {
        // Only an assistant message can be a summary
        if self.role() != Role::Assistant {
            return false;
        }
        self.state
            .marker(&self.msg.id)
            .map(|m| m.is_summary)
            .unwrap_or(false)
    }

    fn estimate_size(&self) -> usize {
        estimate_message(self.msg)
    }

    fn tool_outputs(&self) -> Vec<ToolOutputRef<String>> {
        let Some(api::message::Message::ToolCallResult(tcr)) = &self.msg.message else {
            return Vec::new();
        };
        let tool_name = self
            .tool_names
            .get(&tcr.tool_call_id)
            .cloned()
            .unwrap_or_default();
        let already_compacted = self
            .state
            .marker(&self.msg.id)
            .and_then(|m| m.tool_output_compacted_at)
            .is_some();
        // output_size reuses estimate_message — the ToolCallResult path uses the character count of result/server_message_data
        let output_size = estimate_message(self.msg);
        vec![ToolOutputRef {
            call_id: tcr.tool_call_id.clone(),
            tool_name,
            output_size,
            completed: tcr.result.is_some() || !self.msg.server_message_data.is_empty(),
            already_compacted,
        }]
    }
}

/// Projects a set of messages into a `Vec<WarpMessageView>`, sorted in ascending timestamp order —
/// consistent with the ordering in [`crate::ai::agent_providers::chat_stream::build_chat_request`].
pub fn project<'a>(
    messages: &'a [&'a api::Message],
    state: &'a CompactionState,
    tool_names: &'a ToolNameLookup,
) -> Vec<WarpMessageView<'a>> {
    let mut sorted: Vec<&api::Message> = messages.to_vec();
    sorted.sort_by_key(|m| {
        m.timestamp
            .as_ref()
            .map(|ts| (ts.seconds, ts.nanos))
            .unwrap_or((0, 0))
    });
    sorted
        .into_iter()
        .map(|msg| WarpMessageView {
            msg,
            state,
            tool_names,
        })
        .collect()
}
