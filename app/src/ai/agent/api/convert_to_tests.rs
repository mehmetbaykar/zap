use chrono::{DateTime, Utc};
use warp_core::command::ExitCode;
use warp_multi_agent_api as api;

use crate::ai::agent::task::TaskId;
use crate::ai::agent::{
    AIAgentActionResult, AIAgentActionResultType, AIAgentContext,
    TransferShellCommandControlToUserResult,
};
use crate::terminal::model::block::BlockId;

#[test]
fn git_context_converts_head_and_branch() {
    let context = vec![AIAgentContext::Git {
        head: "abc123".to_string(),
        branch: Some("feature/repo-pr".to_string()),
    }];

    let api_context = super::convert_context(&context);
    let git = api_context.git.expect("expected git context");
    assert_eq!(git.head, "abc123");
    assert_eq!(git.branch, "feature/repo-pr");
}

#[test]
fn transfer_control_snapshot_result_converts_to_tool_call_result_input() {
    let block_id = BlockId::default();
    let input =
        api::request::input::user_inputs::user_input::Input::try_from(AIAgentActionResult {
            id: "tool_call".to_string().into(),
            task_id: TaskId::new("task".to_string()),
            result: AIAgentActionResultType::TransferShellCommandControlToUser(
                TransferShellCommandControlToUserResult::Snapshot {
                    block_id: block_id.clone(),
                    grid_contents: "snapshot".to_string(),
                    cursor: "<|cursor|>".to_string(),
                    is_alt_screen_active: false,
                    is_preempted: false,
                },
            ),
        })
        .unwrap();

    match input {
        api::request::input::user_inputs::user_input::Input::ToolCallResult(result) => {
            assert_eq!(result.tool_call_id, "tool_call");
            match result.result {
                Some(api::request::input::tool_call_result::Result::TransferShellCommandControlToUser(
                    api_result,
                )) => match api_result.result {
                    Some(
                        api::transfer_shell_command_control_to_user_result::Result::LongRunningCommandSnapshot(snapshot),
                    ) => {
                        assert_eq!(snapshot.command_id, block_id.to_string());
                        assert_eq!(snapshot.output, "snapshot");
                        assert_eq!(snapshot.cursor, "<|cursor|>");
                    }
                    other => panic!("Expected snapshot result, got {other:?}"),
                },
                other => panic!("Expected transfer-control tool call result, got {other:?}"),
            }
        }
        other => panic!("Expected tool-call-result input, got {other:?}"),
    }
}

#[test]
fn transfer_control_finished_result_converts_to_tool_call_result_input() {
    let block_id = BlockId::default();
    let start_ts = DateTime::from(Utc::now());
    let completed_ts = DateTime::from(Utc::now());
    let input =
        api::request::input::user_inputs::user_input::Input::try_from(AIAgentActionResult {
            id: "tool_call".to_string().into(),
            task_id: TaskId::new("task".to_string()),
            result: AIAgentActionResultType::TransferShellCommandControlToUser(
                TransferShellCommandControlToUserResult::CommandFinished {
                    block_id: block_id.clone(),
                    output: "done".to_string(),
                    exit_code: ExitCode::from(17),
                    start_ts: Some(start_ts),
                    completed_ts: Some(completed_ts),
                },
            ),
        })
        .unwrap();

    match input {
        api::request::input::user_inputs::user_input::Input::ToolCallResult(result) => {
            assert_eq!(result.tool_call_id, "tool_call");
            match result.result {
                Some(api::request::input::tool_call_result::Result::TransferShellCommandControlToUser(
                    api_result,
                )) => match api_result.result {
                    Some(
                        api::transfer_shell_command_control_to_user_result::Result::CommandFinished(finished),
                    ) => {
                        assert_eq!(finished.command_id, block_id.to_string());
                        assert_eq!(finished.output, "done");
                        assert_eq!(finished.exit_code, 17);
                        // The pinned proto's ShellCommandFinished no longer carries
                        // start/finish timestamps; start_ts/completed_ts stay native-only.
                    }
                    other => panic!("Expected command-finished result, got {other:?}"),
                },
                other => panic!("Expected transfer-control tool call result, got {other:?}"),
            }
        }
        other => panic!("Expected tool-call-result input, got {other:?}"),
    }
}

#[test]
fn start_agent_results_convert_to_versioned_tool_call_results() {
    use ai::agent::action_result::{StartAgentResult, StartAgentVersion};

    let convert = |result: StartAgentResult| {
        let input =
            api::request::input::user_inputs::user_input::Input::try_from(AIAgentActionResult {
                id: "tool_call".to_string().into(),
                task_id: TaskId::new("task".to_string()),
                result: AIAgentActionResultType::StartAgent(result),
            })
            .unwrap();
        match input {
            api::request::input::user_inputs::user_input::Input::ToolCallResult(result) => {
                result.result.expect("start_agent result should be set")
            }
            other => panic!("Expected tool-call-result input, got {other:?}"),
        }
    };

    match convert(StartAgentResult::Success {
        agent_id: "agent-1".to_string(),
        version: StartAgentVersion::V1,
    }) {
        api::request::input::tool_call_result::Result::StartAgent(r) => match r.result {
            Some(api::start_agent_result::Result::Success(s)) => {
                assert_eq!(s.agent_id, "agent-1");
            }
            other => panic!("Expected v1 success, got {other:?}"),
        },
        other => panic!("Expected v1 StartAgent slot, got {other:?}"),
    }

    match convert(StartAgentResult::Cancelled {
        version: StartAgentVersion::V2,
    }) {
        api::request::input::tool_call_result::Result::StartAgentV2(r) => match r.result {
            Some(api::start_agent_v2_result::Result::Error(e)) => {
                assert_eq!(e.error, "Cancelled by user");
            }
            other => panic!("Expected v2 cancelled-as-error, got {other:?}"),
        },
        other => panic!("Expected v2 StartAgentV2 slot, got {other:?}"),
    }
}
