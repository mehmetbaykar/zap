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
fn git_context_converts_repository_and_pull_request_metadata() {
    let context = vec![
        AIAgentContext::Git {
            head: "abc123".to_string(),
            branch: Some("feature/repo-pr".to_string()),
        },
        AIAgentContext::Repository {
            name: "warp-internal".to_string(),
            owner: Some("warpdotdev".to_string()),
            host: Some("github.com".to_string()),
        },
        AIAgentContext::PullRequest {
            number: 42,
            state: "OPEN".to_string(),
            draft: true,
            base_branch: "main".to_string(),
            url: "https://github.com/warpdotdev/warp-internal/pull/42".to_string(),
        },
    ];

    let api_context = super::convert_context(&context);
    let git = api_context.git.expect("expected git context");
    assert_eq!(git.head, "abc123");
    assert_eq!(git.branch, "feature/repo-pr");

    let repository = git.repository.expect("expected repository context");
    assert_eq!(repository.name, "warp-internal");
    assert_eq!(repository.owner, "warpdotdev");
    assert_eq!(repository.host, "github.com");

    let pull_request = git.pull_request.expect("expected pull request context");
    assert_eq!(pull_request.number, 42);
    assert_eq!(
        pull_request.state,
        api::input_context::git::pull_request::State::OpenDraft as i32
    );
    assert_eq!(pull_request.base_branch, "main");
    assert_eq!(
        pull_request.url,
        "https://github.com/warpdotdev/warp-internal/pull/42"
    );
}

#[test]
fn git_context_skips_pull_request_metadata_with_invalid_number() {
    for number in [0, -1] {
        let context = vec![
            AIAgentContext::Git {
                head: "abc123".to_string(),
                branch: Some("feature/repo-pr".to_string()),
            },
            AIAgentContext::PullRequest {
                number,
                state: "OPEN".to_string(),
                draft: false,
                base_branch: "main".to_string(),
                url: "https://github.com/warpdotdev/warp-internal/pull/1".to_string(),
            },
        ];

        let api_context = super::convert_context(&context);
        let git = api_context.git.expect("expected git context");
        assert_eq!(git.head, "abc123");
        assert_eq!(git.branch, "feature/repo-pr");
        assert_eq!(git.pull_request, None);
    }
}

#[test]
fn git_context_skips_pull_request_metadata_with_unknown_state() {
    let context = vec![
        AIAgentContext::Git {
            head: "abc123".to_string(),
            branch: Some("feature/repo-pr".to_string()),
        },
        AIAgentContext::PullRequest {
            number: 42,
            state: "SOMETHING_ELSE".to_string(),
            draft: false,
            base_branch: "main".to_string(),
            url: "https://github.com/warpdotdev/warp-internal/pull/42".to_string(),
        },
    ];

    let api_context = super::convert_context(&context);
    let git = api_context.git.expect("expected git context");
    assert_eq!(git.pull_request, None);
}

#[test]
fn git_context_deserializes_legacy_string_pull_request_number() {
    let pull_request = serde_json::from_str::<AIAgentContext>(
        r#"{"PullRequest":{"number":"42","state":"OPEN","draft":false,"base_branch":"main"}}"#,
    )
    .expect("expected legacy serialized pull request context");

    let api_context = super::convert_context(&[pull_request]);
    let pull_request = api_context
        .git
        .expect("expected git context")
        .pull_request
        .expect("expected pull request context");
    assert_eq!(pull_request.number, 42);
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
