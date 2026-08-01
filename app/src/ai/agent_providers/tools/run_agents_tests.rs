use serde_json::json;
use warp_multi_agent_api as api;

use super::*;

fn run_agents_from_args(args: &str) -> api::RunAgents {
    match (RUN_AGENTS.from_args)(args).expect("from_args should parse") {
        api::message::tool_call::Tool::RunAgents(run_agents) => run_agents,
        other => panic!("expected RunAgents, got {other:?}"),
    }
}

fn launched_result(
    resolved_model_id: &str,
    resolved_harness: Option<api::Harness>,
    agents: Vec<api::run_agents_result::AgentOutcome>,
) -> api::message::tool_call_result::Result {
    api::message::tool_call_result::Result::RunAgentsResult(api::RunAgentsResult {
        outcome: Some(api::run_agents_result::Outcome::Launched(
            api::run_agents_result::Launched {
                resolved_model_id: resolved_model_id.to_string(),
                resolved_harness,
                resolved_execution_mode: Some(
                    api::run_agents_result::launched::ResolvedExecutionMode::Local(
                        api::run_agents::Local {},
                    ),
                ),
                agents,
            },
        )),
    })
}

#[test]
fn from_args_builds_a_local_native_batch() {
    let run_agents = run_agents_from_args(
        r#"{"summary":"split the audit","base_prompt":"repo is at /tmp/zap",
            "agents":[{"name":"Test Fixer","prompt":"fix the tests"},
                      {"name":"Docs Writer","prompt":"update the docs","title":"Docs pass"}]}"#,
    );
    assert_eq!(run_agents.summary, "split the audit");
    assert_eq!(run_agents.base_prompt, "repo is at /tmp/zap");
    // Batch-wide config the BYOP layer intentionally leaves for the card / executor.
    assert!(run_agents.harness.is_none());
    assert!(run_agents.skills.is_empty());
    assert!(run_agents.model_id.is_empty());
    assert!(run_agents.plan_id.is_empty());
    assert_eq!(
        run_agents.execution_mode,
        Some(api::run_agents::ExecutionMode::Local(
            api::run_agents::Local {}
        ))
    );

    assert_eq!(run_agents.agent_run_configs.len(), 2);
    let first = &run_agents.agent_run_configs[0];
    assert_eq!(first.name, "Test Fixer");
    assert_eq!(first.prompt, "fix the tests");
    // Title omitted → falls back to the agent's name.
    assert_eq!(first.title, "Test Fixer");
    assert!(first.agent_identity_uid.is_empty());
    let second = &run_agents.agent_run_configs[1];
    assert_eq!(second.name, "Docs Writer");
    assert_eq!(second.title, "Docs pass");
}

#[test]
fn from_args_passes_harness_through_for_the_whole_batch() {
    let run_agents = run_agents_from_args(
        r#"{"summary":"s","agents":[{"name":"a","prompt":"b"}],"harness":"opencode"}"#,
    );
    assert_eq!(
        run_agents.harness,
        Some(api::Harness {
            variant: Some(api::harness::Variant::OpenCode(api::harness::OpenCode {})),
        })
    );
}

#[test]
fn from_args_whitespace_harness_is_native() {
    let run_agents = run_agents_from_args(
        r#"{"summary":"s","agents":[{"name":"a","prompt":"b"}],"harness":"  "}"#,
    );
    assert!(run_agents.harness.is_none());
}

#[test]
fn from_args_rejects_a_harness_the_local_launcher_cannot_run() {
    // "gemini" and "oz" exist in the proto oneof but `parse_local_child_harness`
    // rejects them, so they must fail here rather than after user approval.
    assert!(
        (RUN_AGENTS.from_args)(
            r#"{"summary":"s","agents":[{"name":"a","prompt":"b"}],"harness":"gemini"}"#
        )
        .is_err()
    );
}

#[test]
fn from_args_rejects_an_empty_batch() {
    assert!((RUN_AGENTS.from_args)(r#"{"summary":"s","agents":[]}"#).is_err());
}

#[test]
fn from_args_rejects_more_agents_than_the_per_call_cap() {
    let agents: Vec<String> = (0..=MAX_AGENTS_PER_CALL)
        .map(|i| format!(r#"{{"name":"a{i}","prompt":"p"}}"#))
        .collect();
    let args = format!(r#"{{"summary":"s","agents":[{}]}}"#, agents.join(","));
    assert!((RUN_AGENTS.from_args)(&args).is_err());
}

#[test]
fn from_args_rejects_duplicate_agent_names() {
    assert!(
        (RUN_AGENTS.from_args)(
            r#"{"summary":"s","agents":[{"name":"a","prompt":"p"},{"name":"a","prompt":"q"}]}"#
        )
        .is_err()
    );
}

#[test]
fn from_args_rejects_a_blank_agent_name() {
    assert!(
        (RUN_AGENTS.from_args)(r#"{"summary":"s","agents":[{"name":"  ","prompt":"p"}]}"#).is_err()
    );
}

#[test]
fn from_args_rejects_missing_prompt() {
    assert!((RUN_AGENTS.from_args)(r#"{"summary":"s","agents":[{"name":"a"}]}"#).is_err());
}

#[test]
fn from_args_rejects_missing_summary() {
    assert!((RUN_AGENTS.from_args)(r#"{"agents":[{"name":"a","prompt":"p"}]}"#).is_err());
}

#[test]
fn result_to_json_reports_each_agent_outcome() {
    let result = launched_result(
        "claude-4-sonnet",
        Some(api::Harness {
            variant: Some(api::harness::Variant::ClaudeCode(
                api::harness::ClaudeCode {},
            )),
        }),
        vec![
            api::run_agents_result::AgentOutcome {
                name: "Test Fixer".to_string(),
                result: Some(api::run_agents_result::agent_outcome::Result::Launched(
                    api::run_agents_result::LaunchedAgent {
                        agent_id: "agent-1".to_string(),
                    },
                )),
            },
            api::run_agents_result::AgentOutcome {
                name: "Docs Writer".to_string(),
                result: Some(api::run_agents_result::agent_outcome::Result::Failed(
                    api::run_agents_result::FailedAgent {
                        error: "harness not installed".to_string(),
                    },
                )),
            },
        ],
    );
    let value = (RUN_AGENTS.result_to_json)(&result).expect("should serialize");
    assert_eq!(
        value,
        json!({
            "status": "launched",
            "model_id": "claude-4-sonnet",
            "harness": "claude",
            "agents": [
                { "name": "Test Fixer", "status": "started", "agent_id": "agent-1" },
                { "name": "Docs Writer", "status": "error", "error": "harness not installed" },
            ]
        })
    );
}

#[test]
fn result_to_json_omits_unresolved_run_wide_config() {
    let result = launched_result(
        "",
        None,
        vec![api::run_agents_result::AgentOutcome {
            name: "a".to_string(),
            result: Some(api::run_agents_result::agent_outcome::Result::Launched(
                api::run_agents_result::LaunchedAgent {
                    agent_id: "agent-1".to_string(),
                },
            )),
        }],
    );
    let value = (RUN_AGENTS.result_to_json)(&result).expect("should serialize");
    assert_eq!(
        value,
        json!({
            "status": "launched",
            "agents": [{ "name": "a", "status": "started", "agent_id": "agent-1" }]
        })
    );
}

#[test]
fn result_to_json_denied() {
    let result = api::message::tool_call_result::Result::RunAgentsResult(api::RunAgentsResult {
        outcome: Some(api::run_agents_result::Outcome::Denied(
            api::run_agents_result::Denied {
                reason: "Duplicate launch rejected.".to_string(),
            },
        )),
    });
    let value = (RUN_AGENTS.result_to_json)(&result).expect("should serialize");
    assert_eq!(
        value,
        json!({ "status": "denied", "reason": "Duplicate launch rejected." })
    );
}

#[test]
fn result_to_json_failure() {
    let result = api::message::tool_call_result::Result::RunAgentsResult(api::RunAgentsResult {
        outcome: Some(api::run_agents_result::Outcome::Failure(
            api::run_agents_result::Failure {
                error: "agent names must be non-empty and unique".to_string(),
            },
        )),
    });
    let value = (RUN_AGENTS.result_to_json)(&result).expect("should serialize");
    assert_eq!(
        value,
        json!({ "status": "error", "error": "agent names must be non-empty and unique" })
    );
}

#[test]
fn result_to_json_cancelled_when_outcome_unset() {
    let result = api::message::tool_call_result::Result::RunAgentsResult(api::RunAgentsResult {
        outcome: None,
    });
    let value = (RUN_AGENTS.result_to_json)(&result).expect("should serialize");
    assert_eq!(value, json!({ "status": "cancelled" }));
}

#[test]
fn result_to_json_ignores_other_variants() {
    // `serialize_result` is first-match-wins across the whole REGISTRY, so claiming
    // a foreign variant here would hijack another tool's result.
    let start_agent =
        api::message::tool_call_result::Result::StartAgentV2(api::StartAgentV2Result {
            result: Some(api::start_agent_v2_result::Result::Success(
                api::start_agent_v2_result::Success {
                    agent_id: "agent-42".to_string(),
                },
            )),
        });
    assert!((RUN_AGENTS.result_to_json)(&start_agent).is_none());

    let server = api::message::tool_call_result::Result::Server(
        api::message::tool_call_result::ServerResult {
            serialized_result: String::new(),
        },
    );
    assert!((RUN_AGENTS.result_to_json)(&server).is_none());
}
