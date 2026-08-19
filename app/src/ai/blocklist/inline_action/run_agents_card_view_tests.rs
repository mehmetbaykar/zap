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
            model_id: String::new(),
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
            resolved_model_id: String::new(),
            kind: RunAgentsAgentOutcomeKind::Launched {
                agent_id: agent_id.to_string(),
            },
        }
    }

    fn failed(name: &str, error: &str) -> RunAgentsAgentOutcome {
        RunAgentsAgentOutcome {
            name: name.to_string(),
            resolved_model_id: String::new(),
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

mod is_orphaned_by_finished_output_tests {
    use super::super::is_orphaned_by_finished_output;
    use crate::ai::agent::{AIAgentOutput, CancellationReason, RenderableAIError, Shared};
    use crate::ai::blocklist::action_model::AIActionStatus;
    use crate::ai::blocklist::block::model::AIBlockOutputStatus;

    fn partial_output() -> Shared<AIAgentOutput> {
        Shared::new(AIAgentOutput::default())
    }

    fn cancelled_block() -> AIBlockOutputStatus {
        AIBlockOutputStatus::Cancelled {
            partial_output: Some(partial_output()),
            reason: CancellationReason::ManuallyCancelled,
        }
    }

    #[test]
    fn statusless_action_on_cancelled_block_is_orphaned() {
        assert!(is_orphaned_by_finished_output(None, &cancelled_block()));
    }

    #[test]
    fn statusless_action_on_failed_block_is_orphaned() {
        let failed = AIBlockOutputStatus::Failed {
            partial_output: Some(partial_output()),
            error: RenderableAIError::other("boom", false),
        };
        assert!(is_orphaned_by_finished_output(None, &failed));
    }

    #[test]
    fn statusless_action_on_unfinished_or_successful_block_is_not_orphaned() {
        for block_status in [
            AIBlockOutputStatus::Pending,
            AIBlockOutputStatus::PartiallyReceived {
                output: partial_output(),
            },
            AIBlockOutputStatus::Complete {
                output: partial_output(),
            },
        ] {
            assert!(
                !is_orphaned_by_finished_output(None, &block_status),
                "{block_status:?} should not orphan the card"
            );
        }
    }

    /// An action that reached the queue gets a real result when the
    /// conversation is cancelled, so its own status must keep driving the card.
    #[test]
    fn action_with_status_on_cancelled_block_is_not_orphaned() {
        for action_status in [
            AIActionStatus::Preprocessing,
            AIActionStatus::Queued,
            AIActionStatus::Blocked,
            AIActionStatus::RunningAsync,
        ] {
            assert!(
                !is_orphaned_by_finished_output(Some(&action_status), &cancelled_block()),
                "{action_status:?} should not orphan the card"
            );
        }
    }
}
