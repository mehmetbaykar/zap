use ai::skills::SkillPathOrigin;

use super::*;
use crate::ai::agent::task::TaskId;

fn convert_tool(tool: api::message::tool_call::Tool) -> MaybeAIAgentAction {
    let task_id = TaskId::new("task-1".to_string());
    let skill_path_origin = SkillPathOrigin::Local;
    let params = ConversionParams {
        task_id: &task_id,
        current_todo_list: None,
        active_code_review: None,
        skill_path_origin: &skill_path_origin,
    };
    api::message::ToolCall {
        tool_call_id: "call-1".to_string(),
        tool: Some(tool),
    }
    .to_action(params)
    .expect("conversion should not error")
}

fn expect_start_agent(action: MaybeAIAgentAction) -> AIAgentActionType {
    match action {
        MaybeAIAgentAction::Action(action) => match action.action {
            start @ AIAgentActionType::StartAgent { .. } => start,
            other => panic!("expected StartAgent action, got {other:?}"),
        },
        MaybeAIAgentAction::Subagent(_) => panic!("expected an action, got Subagent"),
        MaybeAIAgentAction::NoClientRepresentation => {
            panic!("expected an action, got NoClientRepresentation")
        }
    }
}

#[test]
fn start_agent_v1_converts_with_defaults() {
    let action = convert_tool(api::message::tool_call::Tool::StartAgent(api::StartAgent {
        name: "Child".to_string(),
        prompt: "do the thing".to_string(),
        lifecycle_subscription: None,
        execution_mode: None,
    }));
    let AIAgentActionType::StartAgent {
        version,
        name,
        prompt,
        execution_mode,
        lifecycle_subscription,
    } = expect_start_agent(action)
    else {
        unreachable!()
    };
    assert_eq!(version, StartAgentVersion::V1);
    assert_eq!(name, "Child");
    assert_eq!(prompt, "do the thing");
    assert_eq!(
        execution_mode,
        StartAgentExecutionMode::local_with_defaults()
    );
    assert_eq!(lifecycle_subscription, None);
}

#[test]
fn start_agent_v1_remote_mode_degrades_to_local() {
    let action = convert_tool(api::message::tool_call::Tool::StartAgent(api::StartAgent {
        name: "Child".to_string(),
        prompt: "p".to_string(),
        lifecycle_subscription: None,
        execution_mode: Some(api::start_agent::ExecutionMode {
            mode: Some(api::start_agent::execution_mode::Mode::Remote(
                api::start_agent::execution_mode::Remote {
                    environment_id: "env-1".to_string(),
                },
            )),
        }),
    }));
    let AIAgentActionType::StartAgent { execution_mode, .. } = expect_start_agent(action) else {
        unreachable!()
    };
    assert_eq!(
        execution_mode,
        StartAgentExecutionMode::local_with_defaults()
    );
}

#[test]
fn start_agent_v2_converts_harness_and_subscription() {
    let action = convert_tool(api::message::tool_call::Tool::StartAgentV2(
        api::StartAgentV2 {
            name: "Harnessed".to_string(),
            prompt: "p".to_string(),
            lifecycle_subscription: Some(api::start_agent_v2::LifecycleSubscription {
                event_types: vec![
                    api::LifecycleEventType::Errored as i32,
                    api::LifecycleEventType::Succeeded as i32,
                    api::LifecycleEventType::Unspecified as i32,
                    9999,
                ],
            }),
            execution_mode: Some(api::start_agent_v2::ExecutionMode {
                mode: Some(api::start_agent_v2::execution_mode::Mode::Local(
                    api::start_agent_v2::execution_mode::Local {
                        harness: Some(api::start_agent_v2::execution_mode::Harness {
                            r#type: "claude".to_string(),
                        }),
                    },
                )),
            }),
        },
    ));
    let AIAgentActionType::StartAgent {
        version,
        execution_mode,
        lifecycle_subscription,
        ..
    } = expect_start_agent(action)
    else {
        unreachable!()
    };
    assert_eq!(version, StartAgentVersion::V2);
    assert_eq!(
        execution_mode,
        StartAgentExecutionMode::local_harness("claude".to_string())
    );
    // Unspecified and unknown enum values are dropped; known ones map to snake_case names.
    assert_eq!(
        lifecycle_subscription,
        Some(vec!["errored".to_string(), "succeeded".to_string()])
    );
}

#[test]
fn start_agent_v2_blank_harness_is_native_local() {
    let action = convert_tool(api::message::tool_call::Tool::StartAgentV2(
        api::StartAgentV2 {
            name: "n".to_string(),
            prompt: "p".to_string(),
            lifecycle_subscription: Some(api::start_agent_v2::LifecycleSubscription {
                event_types: vec![],
            }),
            execution_mode: Some(api::start_agent_v2::ExecutionMode {
                mode: Some(api::start_agent_v2::execution_mode::Mode::Local(
                    api::start_agent_v2::execution_mode::Local {
                        harness: Some(api::start_agent_v2::execution_mode::Harness {
                            r#type: "   ".to_string(),
                        }),
                    },
                )),
            }),
        },
    ));
    let AIAgentActionType::StartAgent {
        execution_mode,
        lifecycle_subscription,
        ..
    } = expect_start_agent(action)
    else {
        unreachable!()
    };
    assert_eq!(
        execution_mode,
        StartAgentExecutionMode::local_with_defaults()
    );
    // Present-but-empty subscription means "subscribe to none" — preserved as Some([]).
    assert_eq!(lifecycle_subscription, Some(vec![]));
}

#[test]
fn start_agent_v2_remote_mode_degrades_to_local() {
    let action = convert_tool(api::message::tool_call::Tool::StartAgentV2(
        api::StartAgentV2 {
            name: "n".to_string(),
            prompt: "p".to_string(),
            lifecycle_subscription: None,
            execution_mode: Some(api::start_agent_v2::ExecutionMode {
                mode: Some(api::start_agent_v2::execution_mode::Mode::Remote(
                    api::start_agent_v2::execution_mode::Remote::default(),
                )),
            }),
        },
    ));
    let AIAgentActionType::StartAgent { execution_mode, .. } = expect_start_agent(action) else {
        unreachable!()
    };
    assert_eq!(
        execution_mode,
        StartAgentExecutionMode::local_with_defaults()
    );
}
