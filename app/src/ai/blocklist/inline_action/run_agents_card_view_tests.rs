use ai::agent::action::{RunAgentsAgentRunConfig, RunAgentsExecutionMode, RunAgentsRequest};
use ai::agent::action_result::{
    RunAgentsAgentOutcome, RunAgentsAgentOutcomeKind, RunAgentsLaunchedExecutionMode,
    RunAgentsResult,
};
use ai::skills::SkillReference;

use super::RunAgentsEditState;

fn make_request() -> RunAgentsRequest {
    RunAgentsRequest {
        summary: "summary".to_string(),
        base_prompt: "base".to_string(),
        skills: vec![SkillReference::BundledSkillId("create-pr".to_string())],
        model_id: "auto".to_string(),
        harness_type: "oz".to_string(),
        execution_mode: RunAgentsExecutionMode::Local,
        agent_run_configs: vec![RunAgentsAgentRunConfig {
            name: "child".to_string(),
            prompt: "do work".to_string(),
            title: "Child agent".to_string(),
        }],
        plan_id: "plan-1".to_string(),
        harness_auth_secret_name: Some("local-provider-key".to_string()),
    }
}

#[test]
fn request_round_trip_preserves_local_fields() {
    let request = make_request();
    let round_tripped = RunAgentsEditState::from_request(&request).to_request();

    assert_eq!(round_tripped, request);
    assert_eq!(round_tripped.execution_mode, RunAgentsExecutionMode::Local);
}

#[test]
fn edited_harness_and_model_round_trip_locally() {
    let mut state = RunAgentsEditState::from_request(&make_request());
    state.orch.harness_type = "claude".to_string();
    state.orch.model_id = "provider/model".to_string();

    let request = state.to_request();
    assert_eq!(request.harness_type, "claude");
    assert_eq!(request.model_id, "provider/model");
    assert_eq!(request.execution_mode, RunAgentsExecutionMode::Local);
}

mod format_terminal_state_tests {
    use super::super::{StatusKind, format_terminal_state};
    use super::*;

    fn launched(name: &str, agent_id: &str) -> RunAgentsAgentOutcome {
        RunAgentsAgentOutcome {
            name: name.to_string(),
            kind: RunAgentsAgentOutcomeKind::Launched {
                agent_id: agent_id.to_string(),
            },
        }
    }

    fn failed(name: &str, error: &str) -> RunAgentsAgentOutcome {
        RunAgentsAgentOutcome {
            name: name.to_string(),
            kind: RunAgentsAgentOutcomeKind::Failed {
                error: error.to_string(),
            },
        }
    }

    fn launched_result(agents: Vec<RunAgentsAgentOutcome>) -> RunAgentsResult {
        RunAgentsResult::Launched {
            model_id: "auto".to_string(),
            harness_type: "oz".to_string(),
            execution_mode: RunAgentsLaunchedExecutionMode::Local,
            agents,
        }
    }

    #[test]
    fn launched_singular_uses_singular_label() {
        let (label, kind) = format_terminal_state(&launched_result(vec![launched("child", "a-1")]));
        assert_eq!(label, "Spawned 1 agent");
        assert!(matches!(kind, StatusKind::Success));
    }

    #[test]
    fn launched_partial_uses_mixed_status() {
        let result = launched_result(vec![launched("a", "a-1"), failed("b", "boom")]);
        let (label, kind) = format_terminal_state(&result);
        assert_eq!(label, "Spawned 1 of 2 agents");
        assert!(matches!(kind, StatusKind::Mixed));
    }

    #[test]
    fn all_failed_uses_failure_status() {
        let result = launched_result(vec![failed("a", "boom"), failed("b", "boom")]);
        let (label, kind) = format_terminal_state(&result);
        assert_eq!(label, "Failed to spawn 2 agents");
        assert!(matches!(kind, StatusKind::Failure));
    }

    #[test]
    fn failure_includes_local_launch_error() {
        let (label, kind) = format_terminal_state(&RunAgentsResult::Failure {
            error: "local harness missing".to_string(),
        });
        assert_eq!(
            label,
            "Failed to start orchestration: local harness missing"
        );
        assert!(matches!(kind, StatusKind::Failure));
    }

    #[test]
    fn denied_and_cancelled_are_terminal() {
        let (denied_label, denied_kind) = format_terminal_state(&RunAgentsResult::Denied {
            reason: "disabled".to_string(),
        });
        assert!(denied_label.contains("disabled"));
        assert!(matches!(denied_kind, StatusKind::Cancelled));

        let (cancelled_label, cancelled_kind) = format_terminal_state(&RunAgentsResult::Cancelled);
        assert_eq!(cancelled_label, "Spawn agents cancelled");
        assert!(matches!(cancelled_kind, StatusKind::Cancelled));
    }
}
