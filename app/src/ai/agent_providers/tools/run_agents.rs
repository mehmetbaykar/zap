//! Batch child-agent orchestration tool: `run_agents`.
//!
//! Lets the BYOP lead agent spawn a *batch* of local child agents in one call — one
//! `summary` + one shared `base_prompt` + one harness, fanned out over N per-child
//! prompts. This is the fork's only child-orchestration tool; it replaced the
//! single-child `start_agent`, whose protobuf messages upstream deleted. It maps onto
//! the protobuf `RunAgents` tool, so the existing orchestration chain is
//! reused: `convert_from.rs` → `AIAgentActionType::RunAgents(RunAgentsRequest)` →
//! `RunAgentsExecutor` (profile `run_agents` permission + the orchestrate
//! confirmation card) → per-child `StartAgentExecutor::dispatch` → `RunAgentsResult`
//! written back.
//!
//! ## Proto↔action conversion
//!
//! `RunAgents` exists in this fork's pinned `warp_multi_agent_api`
//! (rev `8b219c82a`, `apis/multi_agent/v1/task.proto:1979`) and both directions are
//! wired: `convert_from::convert_run_agents` builds the `RunAgentsRequest`, and
//! `impl From<RunAgentsResult>` in `ai::agent::action_result::convert` maps the
//! outcome back, reached through the `ReqR::RunAgentsResult` arm in
//! `tools::action_result_to_msg_result`.
//!
//! Two proto fields are deliberately not round-tripped, because this fork has no
//! cloud child execution and no skill-path origin at tool-call time: `execution_mode`
//! always resolves to `Local`, and `skills` is always empty.
//!
//! ## Gating (see `chat_stream::build_tools_array` / `available_tool_names`)
//!
//! Listed in `chat_stream::CHILD_ORCHESTRATION_TOOLS`, which is where child
//! orchestration is gated:
//! - `RequestParams.run_agents_enabled` — the active profile's `run_agents`
//!   permission is `NeverAllow` → tool not exposed.
//! - `RequestParams.parent_agent_id.is_some()` — child agents cannot spawn
//!   grandchildren (single-level recursion guard).
//! - Plan Mode blocks it (side-effectful).
//! - `chat_stream` caps calls per assistant turn (`MAX_RUN_AGENTS_CALLS_PER_TURN`);
//!   `from_args` additionally caps children per call (`MAX_AGENTS_PER_CALL`), since
//!   one call already carries a whole batch.

use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use warp_multi_agent_api as api;

use super::OpenAiTool;

pub const TOOL_NAME: &str = "run_agents";

/// Upper bound on `run_agents` calls honored in a single assistant turn.
/// Calls beyond this get a synthetic error result instead of a batch.
///
/// One call per turn, because the real fork-bomb budget is this times
/// `MAX_AGENTS_PER_CALL`. That keeps the per-turn ceiling at four children --
/// the same budget the retired `start_agent` tool allowed (four calls, one child
/// each) -- so replacing it did not quietly widen what one turn can spawn.
pub const MAX_RUN_AGENTS_CALLS_PER_TURN: usize = 1;

/// Upper bound on child agents in ONE `run_agents` call. Enforced in `from_args`
/// (not in `chat_stream`) because the count only exists inside the parsed args;
/// a rejection there surfaces to the model as a normal tool error it can fix.
pub const MAX_AGENTS_PER_CALL: usize = 4;

/// One child agent in the batch.
///
/// The model-facing key is `agents` rather than the proto's `agent_run_configs`:
/// the schema is ours to shape, and shorter names cost fewer tokens per call.
#[derive(Debug, Deserialize)]
struct AgentArgs {
    name: String,
    prompt: String,
    #[serde(default)]
    title: String,
    /// Optional per-child model override. Empty means inherit the batch model.
    #[serde(default)]
    model: String,
}

/// Harness values the local child launcher accepts
/// (`Harness::parse_local_child_harness`): "claude" | "opencode" | "codex".
/// Empty/omitted selects the embedded native child agent, for the whole batch.
#[derive(Debug, Deserialize)]
struct Args {
    summary: String,
    #[serde(default)]
    base_prompt: String,
    agents: Vec<AgentArgs>,
    #[serde(default)]
    harness: String,
}

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": {
                "type": "string",
                "description": "One line describing what the whole batch is for. Shown to the user on the approval card."
            },
            "base_prompt": {
                "type": "string",
                "description": "Shared context prepended to every child's prompt (goal, repository conventions, constraints). Put everything common here instead of repeating it per agent."
            },
            "agents": {
                "type": "array",
                "description": "The child agents to launch together. Each entry gets base_prompt + its own prompt.",
                "minItems": 1,
                "maxItems": MAX_AGENTS_PER_CALL,
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Short display name, unique within this call (e.g. \"Test Fixer\"). Results are correlated back to this name."
                        },
                        "prompt": {
                            "type": "string",
                            "description": "This child's specific task, on top of base_prompt. The child has no access to this conversation's history."
                        },
                        "title": {
                            "type": "string",
                            "description": "Optional title for the spawned agent's task. Defaults to the agent's name."
                        },
                        "model": {
                            "type": "string",
                            "description": "Optional model id for this child only. Omit to use the same model as the rest of the batch."
                        }
                    },
                    "required": ["name", "prompt"],
                    "additionalProperties": false
                }
            },
            "harness": {
                "type": "string",
                "enum": ["claude", "opencode", "codex"],
                "description": "Optional: run every child in the batch inside a third-party CLI harness installed on this machine. Omit to use the built-in native agent."
            }
        },
        "required": ["summary", "agents"],
        "additionalProperties": false
    })
}

/// Maps the model-facing harness string onto the proto `Harness` oneof.
///
/// `None` (omitted / blank) means "no harness selected", which the orchestration
/// executor resolves to the embedded native child agent. `"oz"` and `"gemini"` are
/// deliberately not accepted even though the oneof carries variants for them:
/// `Harness::parse_local_child_harness` rejects both for local children, so taking
/// them here would only produce a batch that fails *after* the user approved it.
fn harness_from_type(harness_type: &str) -> Result<Option<api::Harness>> {
    let variant = match harness_type {
        "" => return Ok(None),
        "claude" => api::harness::Variant::ClaudeCode(api::harness::ClaudeCode {}),
        "opencode" => api::harness::Variant::OpenCode(api::harness::OpenCode {}),
        "codex" => api::harness::Variant::Codex(api::harness::Codex {}),
        other => bail!(
            "unsupported harness '{other}': the local child launcher accepts \
             \"claude\", \"opencode\" or \"codex\"; omit `harness` to use the built-in native agent"
        ),
    };
    Ok(Some(api::Harness {
        variant: Some(variant),
    }))
}

/// Inverse of [`harness_from_type`], for echoing the *resolved* harness back to the
/// model. Uses the same identifiers `convert_from::convert_run_agents_harness` emits,
/// so the converter and this tool speak one vocabulary.
///
/// Visible to the wider crate so `chat_stream` can reuse it when rebuilding a past
/// call's arguments for history replay, rather than keeping a second copy of this
/// mapping in sync.
pub(crate) fn harness_type_from_api(harness: Option<&api::Harness>) -> Option<&'static str> {
    Some(match harness?.variant.as_ref()? {
        api::harness::Variant::Oz(_) => "oz",
        api::harness::Variant::ClaudeCode(_) => "claude",
        api::harness::Variant::OpenCode(_) => "opencode",
        api::harness::Variant::Gemini(_) => "gemini",
        api::harness::Variant::Codex(_) => "codex",
    })
}

fn from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    let parsed: Args = serde_json::from_str(args)?;
    if parsed.agents.is_empty() {
        bail!("run_agents needs at least one entry in `agents`");
    }
    if parsed.agents.len() > MAX_AGENTS_PER_CALL {
        bail!(
            "run_agents accepts at most {} agents per call (got {}); \
             launch the most important ones now and wait for them to finish",
            MAX_AGENTS_PER_CALL,
            parsed.agents.len()
        );
    }
    let harness = harness_from_type(parsed.harness.trim())?;

    // Names are the batch's correlation key: `RunAgentsResult.AgentOutcome` reports
    // per agent by name, and the executor's duplicate-launch guard keys on it too.
    // Blank or repeated names are rejected here rather than after approval, so the
    // model gets a fixable error instead of a half-launched batch. A quadratic scan
    // beats a HashSet at `MAX_AGENTS_PER_CALL` entries.
    for (index, agent) in parsed.agents.iter().enumerate() {
        let name = agent.name.trim();
        if name.is_empty() {
            bail!("every entry in `agents` needs a non-empty `name`");
        }
        if parsed.agents[..index]
            .iter()
            .any(|prior| prior.name.trim() == name)
        {
            bail!("duplicate agent name '{name}': names must be unique within one run_agents call");
        }
    }

    let agent_run_configs = parsed
        .agents
        .into_iter()
        .map(|agent| {
            let name = agent.name.trim().to_owned();
            let title = agent.title.trim();
            api::run_agents::AgentRunConfig {
                // An empty title would leave the spawned task unlabeled; the agent's
                // own name is the obvious fallback.
                title: if title.is_empty() {
                    name.clone()
                } else {
                    title.to_owned()
                },
                name,
                prompt: agent.prompt,
                // Cloud-only: identifies the service account a factory agent
                // dispatches siblings as. Zap runs children locally, as the user.
                agent_identity_uid: String::new(),
                // Per-child overrides. The model is exposed to the caller; harness
                // and execution mode stay batch-level in this fork, so they are left
                // unset rather than claiming a per-child choice was made.
                model_id: agent.model.trim().to_owned(),
                harness: None,
                execution_mode: None,
            }
        })
        .collect();

    Ok(api::message::tool_call::Tool::RunAgents(api::RunAgents {
        summary: parsed.summary,
        base_prompt: parsed.base_prompt,
        // Skills are resolved against a `SkillPathOrigin` the BYOP layer does not
        // have at tool-call time; the lead agent uses `read_skill` instead.
        skills: Vec::new(),
        // Run-wide model is a user choice made on the confirmation card (empty =
        // children inherit the lead agent's model). Not exposed to the model, which
        // would only hallucinate provider-specific ids.
        model_id: String::new(),
        harness,
        agent_run_configs,
        // Only set for batches that inherit an approved OrchestrationConfigSnapshot
        // from a plan document. A BYOP call never has one, and a non-empty value
        // here would make `RunAgentsExecutor::should_autoexecute` look for a plan
        // approval that does not exist.
        plan_id: String::new(),
        // Zap has no cloud: children always run locally. `RunAgentsExecutionMode`
        // has no Remote variant, and the executor denies remote batches outright.
        execution_mode: Some(api::run_agents::ExecutionModeOneOf::Local(
            api::run_agents::Local {},
        )),
    }))
}

fn result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    // Note the naming asymmetry: the call side is `Tool::RunAgents`, the result side
    // is `Result::RunAgentsResult`. Every other variant must return `None` —
    // `tools::serialize_result` walks the whole REGISTRY first-match-wins, so a
    // greedy arm here would hijack other tools' results.
    let R::RunAgentsResult(run_agents_result) = result else {
        return None;
    };
    let value = match &run_agents_result.outcome {
        Some(api::run_agents_result::Outcome::Launched(launched)) => {
            let agents: Vec<Value> = launched
                .agents
                .iter()
                .map(|agent| {
                    use api::run_agents_result::agent_outcome::Result as AgentResult;
                    match &agent.result {
                        Some(AgentResult::Launched(l)) => json!({
                            "name": agent.name,
                            "status": "started",
                            "agent_id": l.agent_id,
                        }),
                        Some(AgentResult::Failed(f)) => json!({
                            "name": agent.name,
                            "status": "error",
                            "error": f.error,
                        }),
                        // Outcome oneof unset: the batch launched but this child's
                        // fate is unknown. Report it rather than silently dropping
                        // the entry, so the model's count still matches its request.
                        None => json!({ "name": agent.name, "status": "unknown" }),
                    }
                })
                .collect();
            let mut out = json!({ "status": "launched", "agents": agents });
            // The resolved run-wide config is echoed only when the executor filled
            // it in — the user can change model/harness on the confirmation card, so
            // what ran is not necessarily what was asked for.
            if !launched.resolved_model_id.is_empty() {
                out["model_id"] = json!(launched.resolved_model_id);
            }
            if let Some(harness) = harness_type_from_api(launched.resolved_harness.as_ref()) {
                out["harness"] = json!(harness);
            }
            out
        }
        // Declined for a non-error reason (permission set to never allow, duplicate
        // batch, "accept without orchestration"). Distinct from a failure so the
        // model stops asking instead of retrying.
        Some(api::run_agents_result::Outcome::Denied(denied)) => {
            json!({ "status": "denied", "reason": denied.reason })
        }
        Some(api::run_agents_result::Outcome::Failure(failure)) => {
            json!({ "status": "error", "error": failure.error })
        }
        None => json!({ "status": "cancelled" }),
    };
    Some(value)
}

pub static RUN_AGENTS: OpenAiTool = OpenAiTool {
    name: TOOL_NAME,
    description: include_str!("../prompts/tool_descriptions/run_agents.md"),
    parameters,
    from_args,
    result_to_json,
};

#[cfg(test)]
#[path = "run_agents_tests.rs"]
mod tests;
