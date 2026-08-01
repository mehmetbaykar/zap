use super::*;
use crate::agent::action_result::{RunAgentsResult, WaitForEventsResult};

fn local_run_request() -> RunAgentsRequest {
    RunAgentsRequest {
        summary: "Inspect and fix".to_string(),
        base_prompt: "Work locally".to_string(),
        skills: Vec::new(),
        model_id: "local-model".to_string(),
        harness_type: "codex".to_string(),
        execution_mode: RunAgentsExecutionMode::Local,
        agent_run_configs: vec![RunAgentsAgentRunConfig {
            name: "worker".to_string(),
            prompt: "Inspect the code".to_string(),
            title: "Code inspection".to_string(),
            model_id: String::new(),
        }],
        plan_id: "plan-1".to_string(),
        harness_auth_secret_name: Some("codex-auth".to_string()),
    }
}

#[test]
fn local_run_agents_action_has_native_cancelled_result() {
    let action = AIAgentActionType::RunAgents(local_run_request());

    assert_eq!(action.user_friendly_name(), "Orchestrate 1 agent(s)");
    assert_eq!(action.presence_continuous_summary(), "Orchestrating agents");
    assert_eq!(
        action.cancelled_result(),
        AIAgentActionResultType::RunAgents(RunAgentsResult::Cancelled)
    );
}

#[test]
fn wait_for_events_action_preserves_local_correlation_fields() {
    let action = AIAgentActionType::WaitForEvents(WaitForEventsRequest {
        tool_call_id: "tool-1".to_string(),
        idle_timeout_seconds: 30,
    });

    assert_eq!(
        action.to_string(),
        "WaitForEvents: tool_call_id=tool-1 idle_timeout_seconds=30"
    );
    assert_eq!(
        action.cancelled_result(),
        AIAgentActionResultType::WaitForEvents(WaitForEventsResult::Cancelled)
    );
}
