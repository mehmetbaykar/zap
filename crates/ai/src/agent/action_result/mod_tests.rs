use super::*;

#[test]
fn local_run_agents_result_reports_ordered_outcomes() {
    let result = AIAgentActionResultType::RunAgents(RunAgentsResult::Launched {
        model_id: "local-model".to_string(),
        harness_type: "codex".to_string(),
        execution_mode: RunAgentsLaunchedExecutionMode::Local,
        agents: vec![
            RunAgentsAgentOutcome {
                name: "first".to_string(),
                kind: RunAgentsAgentOutcomeKind::Launched {
                    agent_id: "agent-1".to_string(),
                },
            },
            RunAgentsAgentOutcome {
                name: "second".to_string(),
                kind: RunAgentsAgentOutcomeKind::Failed {
                    error: "not installed".to_string(),
                },
            },
        ],
    });

    assert!(result.is_successful());
    assert!(!result.is_failed());
    assert!(!result.is_cancelled());
    assert_eq!(
        result.to_string(),
        "Orchestrate launched (1/2 agents started)"
    );
}

#[test]
fn local_orchestration_terminal_states_are_classified() {
    let denied = AIAgentActionResultType::RunAgents(RunAgentsResult::Denied {
        reason: "not approved".to_string(),
    });
    let cancelled = AIAgentActionResultType::RunAgents(RunAgentsResult::Cancelled);
    let wait_completed = AIAgentActionResultType::WaitForEvents(WaitForEventsResult::Completed);
    let wait_cancelled = AIAgentActionResultType::WaitForEvents(WaitForEventsResult::Cancelled);

    assert!(denied.is_failed());
    assert!(cancelled.is_cancelled());
    assert!(wait_completed.is_successful());
    assert!(wait_cancelled.is_cancelled());
}
