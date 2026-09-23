use std::collections::HashMap;

use ai::agent::action_result::{RunAgentsAgentOutcomeKind, RunAgentsResult};
use warp_multi_agent_api as api;

use super::*;

#[test]
#[allow(deprecated)]
fn test_convert_tool_call_result_to_input_run_agents_launched() {
    let tool_call_result = api::message::ToolCallResult {
        tool_call_id: "run-agents-call".to_string(),
        result: Some(api::message::tool_call_result::Result::RunAgentsResult(
            api::RunAgentsResult {
                outcome: Some(api::run_agents_result::Outcome::Launched(
                    api::run_agents_result::Launched {
                        resolved_model_id: "batch-model".to_string(),
                        agents: vec![api::run_agents_result::AgentOutcome {
                            name: "researcher".to_string(),
                            model_id: "child-model".to_string(),
                            result: Some(api::run_agents_result::agent_outcome::Result::Launched(
                                api::run_agents_result::LaunchedAgent {
                                    agent_id: "agent-1".to_string(),
                                },
                            )),
                            ..Default::default()
                        }],
                        resolved_execution_mode: Some(
                            api::run_agents_result::launched::ResolvedExecutionMode::Local(
                                api::run_agents::Local {},
                            ),
                        ),
                        ..Default::default()
                    },
                )),
            },
        )),
        ..Default::default()
    };

    let input = convert_tool_call_result_to_input(
        &TaskId::new("task".to_string()),
        &tool_call_result,
        &HashMap::new(),
        &mut HashMap::new(),
    );

    let Some(AIAgentInput::ActionResult { result, .. }) = input else {
        panic!("expected a run_agents action result, got {input:?}");
    };
    let AIAgentActionResultType::RunAgents(RunAgentsResult::Launched {
        model_id, agents, ..
    }) = result.result
    else {
        panic!("expected a launched run_agents result");
    };
    assert_eq!(model_id, "batch-model");
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "researcher");
    assert_eq!(agents[0].resolved_model_id, "child-model");
    assert_eq!(
        agents[0].kind,
        RunAgentsAgentOutcomeKind::Launched {
            agent_id: "agent-1".to_string()
        }
    );
}

#[test]
fn test_convert_tool_call_result_to_input_run_agents_denied() {
    let tool_call_result = api::message::ToolCallResult {
        tool_call_id: "run-agents-call".to_string(),
        result: Some(api::message::tool_call_result::Result::RunAgentsResult(
            api::RunAgentsResult {
                outcome: Some(api::run_agents_result::Outcome::Denied(
                    api::run_agents_result::Denied {
                        reason: "Cancelled by user".to_string(),
                    },
                )),
            },
        )),
        ..Default::default()
    };

    let input = convert_tool_call_result_to_input(
        &TaskId::new("task".to_string()),
        &tool_call_result,
        &HashMap::new(),
        &mut HashMap::new(),
    );

    assert!(matches!(
        input,
        Some(AIAgentInput::ActionResult {
            result: AIAgentActionResult {
                result: AIAgentActionResultType::RunAgents(RunAgentsResult::Denied { .. }),
                ..
            },
            ..
        })
    ));
}
