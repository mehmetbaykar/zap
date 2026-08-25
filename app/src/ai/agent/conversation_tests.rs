use std::collections::HashMap;

use warp_core::features::FeatureFlag;
use warp_multi_agent_api as api;

use super::{
    AIConversation, AIConversationAutoexecuteMode, AIConversationId, ConversationStatus,
    RestoreConversationError, TaskId, artifact_from_fork_proto,
};
use crate::ai::artifacts::Artifact;
use crate::ai::blocklist::SerializedBlockListItem;
use crate::ai::blocklist::block::cli_controller::LongRunningCommandControlState;
use crate::ai::byop_readiness::{
    InvalidRepairState, RepairRecord, RepairSource, RepairState, RepairStateLoadError,
    RepairStateStatus, ToolCallKey,
};
use crate::persistence::ModelEvent;
use crate::persistence::model::{AgentConversationData, ChargedUsageTotals};
use crate::terminal::model::BlockId;
use crate::terminal::model::block::{
    AgentInteractionMetadata, AgentViewVisibility, SerializedAIMetadata, SerializedBlock,
};

fn restored_conversation(conversation_data: Option<AgentConversationData>) -> AIConversation {
    AIConversation::new_restored(
        AIConversationId::new(),
        vec![api::Task {
            id: "root-task".to_string(),
            messages: vec![],
            dependencies: None,
            description: String::new(),
            summary: String::new(),
            server_data: String::new(),
        }],
        conversation_data,
    )
    .unwrap()
}

fn restored_conversation_with_root_description(description: &str) -> AIConversation {
    AIConversation::new_restored(
        AIConversationId::new(),
        vec![api::Task {
            id: "root-task".to_string(),
            messages: vec![],
            dependencies: None,
            description: description.to_string(),
            summary: String::new(),
            server_data: String::new(),
        }],
        None,
    )
    .unwrap()
}

fn user_query_message(id: &str, request_id: &str, query: &str) -> api::Message {
    api::Message {
        fetched_memories: Vec::new(),
        id: id.to_string(),
        task_id: "root-task".to_string(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::UserQuery(api::message::UserQuery {
            query: query.to_string(),
            context: None,
            referenced_attachments: HashMap::new(),
            mode: None,
            intended_agent: Default::default(),
        })),
        request_id: request_id.to_string(),
        timestamp: None,
    }
}

#[allow(deprecated)]
fn user_query_message_with_shell_context(
    id: &str,
    request_id: &str,
    query: &str,
    attachment_command: &str,
    context_command: &str,
) -> api::Message {
    let mut referenced_attachments = HashMap::new();
    referenced_attachments.insert(
        "attachment-command".to_string(),
        api::Attachment {
            value: Some(api::attachment::Value::ExecutedShellCommand(
                api::ExecutedShellCommand {
                    command: attachment_command.to_string(),
                    output: "attachment output".to_string(),
                    exit_code: 0,
                    command_id: "attachment-block".to_string(),
                    is_auto_attached: false,
                    started_ts: None,
                    finished_ts: None,
                },
            )),
        },
    );

    api::Message {
        fetched_memories: Vec::new(),
        id: id.to_string(),
        task_id: "root-task".to_string(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::UserQuery(api::message::UserQuery {
            query: query.to_string(),
            context: Some(api::InputContext {
                executed_shell_commands: vec![api::ExecutedShellCommand {
                    command: context_command.to_string(),
                    output: "context output".to_string(),
                    exit_code: 0,
                    command_id: "context-block".to_string(),
                    is_auto_attached: false,
                    started_ts: None,
                    finished_ts: None,
                }],
                ..Default::default()
            }),
            referenced_attachments,
            mode: None,
            intended_agent: Default::default(),
        })),
        request_id: request_id.to_string(),
        timestamp: None,
    }
}

fn tool_call_result_message(
    id: &str,
    request_id: &str,
    tool_call_id: &str,
    result: api::message::tool_call_result::Result,
) -> api::Message {
    api::Message {
        fetched_memories: vec![],
        id: id.to_string(),
        task_id: "root-task".to_string(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::ToolCallResult(
            api::message::ToolCallResult {
                tool_call_id: tool_call_id.to_string(),
                context: None,
                result: Some(result),
            },
        )),
        request_id: request_id.to_string(),
        timestamp: None,
    }
}

fn start_recording_tool_call() -> api::message::tool_call::Tool {
    api::message::tool_call::Tool::StartRecording(api::message::tool_call::StartRecording {
        frame_rate: 15,
        limits: None,
        summary: String::new(),
        playback_speed_multiplier: 0,
        target: None,
        description: String::new(),
    })
}

fn start_recording_success_result(recording_id: &str) -> api::message::tool_call_result::Result {
    api::message::tool_call_result::Result::StartRecording(api::StartRecordingResult {
        result: Some(api::start_recording_result::Result::Success(
            api::start_recording_result::Success {
                recording_id: recording_id.to_string(),
                started_at: None,
                settings: Some(api::start_recording_result::CaptureSettings {
                    width_px: 1280,
                    height_px: 720,
                }),
            },
        )),
    })
}

fn start_recording_error_result(message: &str) -> api::message::tool_call_result::Result {
    api::message::tool_call_result::Result::StartRecording(api::StartRecordingResult {
        result: Some(api::start_recording_result::Result::Error(
            api::start_recording_result::Error {
                message: message.to_string(),
            },
        )),
    })
}

fn use_computer_tool_call(summary: &str) -> api::message::tool_call::Tool {
    api::message::tool_call::Tool::UseComputer(api::message::tool_call::UseComputer {
        actions: vec![],
        post_actions_screenshot_params: None,
        action_summary: summary.to_string(),
    })
}

fn stop_recording_tool_call(recording_id: &str) -> api::message::tool_call::Tool {
    api::message::tool_call::Tool::StopRecording(api::message::tool_call::StopRecording {
        recording_id: recording_id.to_string(),
        discard: false,
    })
}

fn stop_recording_success_result(artifact_uid: &str) -> api::message::tool_call_result::Result {
    api::message::tool_call_result::Result::StopRecording(api::StopRecordingResult {
        result: Some(api::stop_recording_result::Result::Success(
            api::stop_recording_result::Success {
                artifact_uid: artifact_uid.to_string(),
                duration: Some(prost_types::Duration {
                    seconds: 2,
                    nanos: 0,
                }),
                width_px: 1280,
                height_px: 720,
                size_bytes: 42,
                completion_status: api::stop_recording_result::CompletionStatus::Complete as i32,
                termination_reason: "Stopped by agent".to_string(),
            },
        )),
    })
}

fn stop_recording_error_result(message: &str) -> api::message::tool_call_result::Result {
    api::message::tool_call_result::Result::StopRecording(api::StopRecordingResult {
        result: Some(api::stop_recording_result::Result::Error(
            api::stop_recording_result::Error {
                message: message.to_string(),
            },
        )),
    })
}
fn restored_conversation_with_messages(messages: Vec<api::Message>) -> AIConversation {
    AIConversation::new_restored(
        AIConversationId::new(),
        vec![api::Task {
            id: "root-task".to_string(),
            messages,
            dependencies: None,
            description: String::new(),
            summary: String::new(),
            server_data: String::new(),
        }],
        None,
    )
    .unwrap()
}

fn agent_output_message(id: &str, request_id: &str) -> api::Message {
    api::Message {
        fetched_memories: Vec::new(),
        id: id.to_string(),
        task_id: "root-task".to_string(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::AgentOutput(
            api::message::AgentOutput {
                text: "Done".to_string(),
            },
        )),
        request_id: request_id.to_string(),
        timestamp: None,
    }
}

fn tool_call_message(id: &str, call_id: &str) -> api::Message {
    api::Message {
        fetched_memories: Vec::new(),
        id: id.to_string(),
        task_id: "root-task".to_string(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::ToolCall(api::message::ToolCall {
            tool_call_id: call_id.to_string(),
            tool: None,
        })),
        request_id: "request-1".to_string(),
        timestamp: None,
    }
}

fn run_shell_command_tool() -> api::message::tool_call::Tool {
    use api::message::tool_call::run_shell_command::WaitUntilCompleteValue;

    api::message::tool_call::Tool::RunShellCommand(api::message::tool_call::RunShellCommand {
        command: "echo hi".to_string(),
        is_read_only: true,
        uses_pager: false,
        is_risky: false,
        citations: vec![],
        wait_until_complete_value: Some(WaitUntilCompleteValue::WaitUntilComplete(true)),
        risk_category: 0,
    })
}

fn tool_call_message_with_tool(
    id: &str,
    call_id: &str,
    tool: api::message::tool_call::Tool,
) -> api::Message {
    api::Message {
        fetched_memories: Vec::new(),
        id: id.to_string(),
        task_id: "root-task".to_string(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::ToolCall(api::message::ToolCall {
            tool_call_id: call_id.to_string(),
            tool: Some(tool),
        })),
        request_id: "request-1".to_string(),
        timestamp: None,
    }
}

fn tool_call_result_message_with_result(
    id: &str,
    call_id: &str,
    result: api::message::tool_call_result::Result,
) -> api::Message {
    api::Message {
        fetched_memories: Vec::new(),
        id: id.to_string(),
        task_id: "root-task".to_string(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::ToolCallResult(
            api::message::ToolCallResult {
                tool_call_id: call_id.to_string(),
                result: Some(result),
                context: None,
            },
        )),
        request_id: "request-1".to_string(),
        timestamp: None,
    }
}

fn cli_subagent_tool(subtask_id: &str, command_id: &str) -> api::message::tool_call::Tool {
    api::message::tool_call::Tool::Subagent(api::message::tool_call::Subagent {
        task_id: subtask_id.to_string(),
        payload: String::new(),
        metadata: Some(api::message::tool_call::subagent::Metadata::Cli(
            api::message::tool_call::subagent::CliSubagent {
                command_id: command_id.to_string(),
            },
        )),
    })
}

fn empty_agent_conversation_data_for_test() -> AgentConversationData {
    AgentConversationData {
        server_conversation_token: None,
        conversation_usage_metadata: None,
        reverted_action_ids: None,
        forked_from_server_conversation_token: None,
        artifacts_json: None,
        parent_agent_id: None,
        agent_name: None,
        orchestration_harness_type: None,
        parent_conversation_id: None,
        root_task_is_optimistic: None,
        run_id: None,
        autoexecute_override: None,
        last_event_sequence: None,
        compaction_state_json: None,
        byop_repair_state_json: None,
        cli_subagent_block_snapshots_json: None,
        pinned: false,
        is_remote_child: false,
    }
}

fn cli_subagent_snapshot_json_for_test(
    conversation_id: AIConversationId,
    task_id: &TaskId,
    block_id: &BlockId,
    command: &[u8],
    output: &[u8],
) -> String {
    // Simulate the only terminal snapshot that survives in SQLite conversation_data after a tab closes.
    let mut block = SerializedBlock::new_for_test(command.to_vec(), output.to_vec());
    block.id = block_id.clone();
    block.ai_metadata = serde_json::to_string(&Some(Into::<SerializedAIMetadata>::into(
        AgentInteractionMetadata::new(
            None,
            conversation_id,
            Some(task_id.clone()),
            Some(LongRunningCommandControlState::Agent {
                is_blocked: false,
                should_hide_responses: false,
            }),
            false,
            false,
        ),
    )))
    .ok();
    block.agent_view_visibility =
        Some(AgentViewVisibility::new_from_conversation(conversation_id).into());

    serde_json::to_string(&vec![serde_json::json!({
        "task_id": String::from(task_id.clone()),
        "block_id": block_id.as_str(),
        "block": block,
    })])
    .expect("CLI subagent snapshot JSON should serialize")
}

fn restored_conversation_with_queries(queries: &[&str]) -> AIConversation {
    let messages = queries
        .iter()
        .enumerate()
        .flat_map(|(index, query)| {
            let request_id = format!("request-{index}");
            [
                user_query_message(&format!("user-{index}"), &request_id, query),
                agent_output_message(&format!("agent-{index}"), &request_id),
            ]
        })
        .collect();

    AIConversation::new_restored(
        AIConversationId::new(),
        vec![api::Task {
            id: "root-task".to_string(),
            messages,
            dependencies: None,
            description: String::new(),
            summary: String::new(),
            server_data: String::new(),
        }],
        None,
    )
    .unwrap()
}

#[test]
fn latest_user_query_returns_latest_non_empty_user_query() {
    let conversation =
        restored_conversation_with_queries(&["write unit tests", "fix the failing test"]);

    assert_eq!(
        conversation.latest_user_query(),
        Some("fix the failing test".to_string())
    );
}

#[test]
fn latest_user_query_trims_and_skips_empty_queries() {
    let conversation = restored_conversation_with_queries(&["  write unit tests  ", "  "]);

    assert_eq!(
        conversation.latest_user_query(),
        Some("write unit tests".to_string())
    );
}

#[test]
fn title_uses_root_task_description() {
    let conversation = restored_conversation_with_root_description("Root task title");

    assert_eq!(conversation.title().as_deref(), Some("Root task title"));
}

#[test]
fn title_falls_back_to_initial_query_when_root_description_is_empty() {
    let conversation = restored_conversation_with_queries(&["Initial query"]);

    assert_eq!(conversation.title().as_deref(), Some("Initial query"));
}

#[test]
fn reassign_exchange_ids_keeps_exchange_lookup_consistent() {
    let mut conversation = restored_conversation_with_queries(&["one", "two"]);

    let old_ids: Vec<_> = conversation.all_exchanges().iter().map(|e| e.id).collect();
    assert!(!old_ids.is_empty());

    // Pre-condition: every original id resolves via the exchange-id index.
    for id in &old_ids {
        assert!(conversation.exchange_with_id(*id).is_some());
    }

    conversation.reassign_exchange_ids();

    // Reassigning regenerates ids without changing the exchange count, so
    // `modify_task` does not rebuild the index; correctness relies on the
    // explicit `rebuild_exchange_index()` call. The stale ids must be gone.
    for id in &old_ids {
        assert!(conversation.exchange_with_id(*id).is_none());
    }

    // Every current id resolves via the rebuilt index.
    let new_ids: Vec<_> = conversation.all_exchanges().iter().map(|e| e.id).collect();
    assert_eq!(new_ids.len(), old_ids.len());
    for id in &new_ids {
        assert!(conversation.exchange_with_id(*id).is_some());
    }
}

#[test]
fn restored_conversation_defaults_autoexecute_override_when_not_persisted() {
    let _flag = FeatureFlag::RememberFastForwardState.override_enabled(true);
    let conversation_data: AgentConversationData =
        serde_json::from_str(r#"{"server_conversation_token":null}"#).unwrap();

    let conversation = restored_conversation(Some(conversation_data));

    assert_eq!(
        conversation.autoexecute_override(),
        AIConversationAutoexecuteMode::RespectUserSettings
    );
}

#[test]
fn restored_conversation_uses_persisted_last_event_sequence() {
    let conversation_data: AgentConversationData =
        serde_json::from_str(r#"{"server_conversation_token":null,"last_event_sequence":42}"#)
            .unwrap();

    let conversation = restored_conversation(Some(conversation_data));

    assert_eq!(conversation.last_event_sequence(), Some(42));
}

#[test]
fn restored_conversation_uses_persisted_remote_child_marker() {
    let conversation_data: AgentConversationData =
        serde_json::from_str(r#"{"server_conversation_token":null,"is_remote_child":true}"#)
            .unwrap();

    let conversation = restored_conversation(Some(conversation_data));

    assert!(conversation.is_remote_child());
}

#[test]
fn child_conversation_detection_uses_parent_agent_id() {
    let conversation_data: AgentConversationData = serde_json::from_str(
        r#"{"server_conversation_token":null,"parent_agent_id":"parent-run-id"}"#,
    )
    .unwrap();

    let conversation = restored_conversation(Some(conversation_data));

    assert!(conversation.is_child_agent_conversation());
    assert_eq!(conversation.parent_conversation_id(), None);
}

/// When the persisted task list is empty (e.g. a child conversation persisted
/// before any server response), restoring via `new_restored_synthesizing_on_empty`
/// must produce a fresh in-progress optimistic root, mirroring
/// `AIConversation::new()`.
#[test]
fn restored_conversation_with_empty_task_list_creates_in_progress_optimistic_root() {
    let conversation =
        AIConversation::new_restored_synthesizing_on_empty(AIConversationId::new(), vec![], None)
            .expect("empty task list must synthesize an optimistic root");

    let root_task = conversation
        .get_root_task()
        .expect("synthesized root task should exist");
    assert!(root_task.is_root_task());
    assert!(
        root_task.source().is_none(),
        "synthesized root is optimistic and has no api::Task source"
    );
    assert!(
        !root_task.id().to_string().is_empty(),
        "synthesized optimistic root must have a non-empty UUID id"
    );
    assert_eq!(conversation.status(), &ConversationStatus::InProgress);
    assert!(conversation.status_error_message().is_none());
}

/// A user-initiated turn starts a new response block, so the previous
/// block's charged-usage breakdown must be cleared even when the new turn
/// carries no `request_charges` of its own — otherwise a fresh credits
/// figure would be paired with the previous block's stale token/cost
/// details.
#[test]
fn update_cost_and_usage_resets_stale_charged_usage_for_last_block_on_new_user_turn() {
    let mut conversation = AIConversation::new(false, false);
    // Simulate a stale last-block breakdown left over from a previous
    // response, as happens whenever this turn's request carries no
    // `request_charges` (BYOP never populates them).
    conversation.set_charged_usage_for_last_block_for_test(Some(ChargedUsageTotals {
        input_tokens: 500,
        ..Default::default()
    }));

    conversation
        .update_cost_and_usage_for_request(None, None, vec![], None, true)
        .expect("usage should update");

    assert_eq!(
        conversation.charged_usage_for_last_block(),
        None,
        "a new user-initiated turn must clear the previous block's stale charged usage, \
         even when this turn's request itself carries no charges"
    );
}

/// The legacy `AgentConversationData.root_task_is_optimistic` flag must be
/// ignored on restore. A non-empty task list always produces a real
/// server-backed root regardless of whether the flag is set.
#[test]
fn restored_conversation_ignores_legacy_root_task_is_optimistic_flag_with_non_empty_tasks() {
    let conversation_data: AgentConversationData = serde_json::from_str(
        r#"{"server_conversation_token":null,"root_task_is_optimistic":true}"#,
    )
    .unwrap();

    let conversation = restored_conversation(Some(conversation_data));
    let root_task = conversation
        .get_root_task()
        .expect("root task should exist");

    assert_eq!(root_task.id().to_string(), "root-task");
    assert!(root_task.is_root_task());
    assert!(
        root_task.source().is_some(),
        "with a real task list, the legacy optimistic flag must be ignored",
    );
}

/// The legacy `root_task_is_optimistic` flag is ignored when restoring an
/// empty task list via `new_restored_synthesizing_on_empty`.
#[test]
fn restored_conversation_ignores_legacy_root_task_is_optimistic_flag_with_empty_tasks() {
    let conversation_data: AgentConversationData = serde_json::from_str(
        r#"{"server_conversation_token":null,"root_task_is_optimistic":true}"#,
    )
    .unwrap();

    let conversation = AIConversation::new_restored_synthesizing_on_empty(
        AIConversationId::new(),
        vec![],
        Some(conversation_data),
    )
    .expect("empty task list must synthesize an optimistic root regardless of legacy flag");

    let root_task = conversation
        .get_root_task()
        .expect("synthesized root task should exist");
    assert!(root_task.is_root_task());
    assert!(root_task.source().is_none());
    assert_eq!(conversation.status(), &ConversationStatus::InProgress);
}

/// Strict `new_restored` returns `NoRootTask` for an empty task list.
#[test]
fn new_restored_with_empty_task_list_returns_no_root_task_error() {
    let result = AIConversation::new_restored(AIConversationId::new(), vec![], None);
    assert!(
        matches!(result, Err(RestoreConversationError::NoRootTask)),
        "empty task list via strict new_restored must return NoRootTask; got {result:?}",
    );
}

/// When multiple parentless tasks exist (e.g. a legacy orphan optimistic
/// stub alongside the real server root), `new_restored` must prefer the
/// candidate whose `messages` is non-empty. Each ordering runs in a loop to
/// surface any nondeterminism in candidate selection.
#[test]
fn test_new_restored_prefers_parentless_task_with_messages_over_empty_stub() {
    let stub = api::Task {
        id: "optimistic-stub-uuid".to_string(),
        messages: vec![],
        dependencies: None,
        description: String::new(),
        summary: String::new(),
        server_data: String::new(),
    };
    let real = api::Task {
        id: "server-root-id".to_string(),
        messages: vec![user_query_message("user-msg", "request-1", "real query")],
        dependencies: None,
        description: String::new(),
        summary: String::new(),
        server_data: String::new(),
    };

    // Stub appears first in the vec.
    for _ in 0..50 {
        let conversation = AIConversation::new_restored(
            AIConversationId::new(),
            vec![stub.clone(), real.clone()],
            None,
        )
        .expect("restore with stub + real parentless tasks must succeed");
        let root_task = conversation
            .get_root_task()
            .expect("restored conversation must have a root task");
        assert_eq!(
            root_task.id().to_string(),
            "server-root-id",
            "expected the real (non-empty) parentless task to win when stub is first",
        );
        let source = root_task
            .source()
            .expect("chosen root must have api::Task source");
        assert!(
            !source.messages.is_empty(),
            "chosen root must have non-empty messages",
        );
    }

    // Real appears first in the vec.
    for _ in 0..50 {
        let conversation = AIConversation::new_restored(
            AIConversationId::new(),
            vec![real.clone(), stub.clone()],
            None,
        )
        .expect("restore with real + stub parentless tasks must succeed");
        let root_task = conversation
            .get_root_task()
            .expect("restored conversation must have a root task");
        assert_eq!(
            root_task.id().to_string(),
            "server-root-id",
            "expected the real (non-empty) parentless task to win when real is first",
        );
        let source = root_task
            .source()
            .expect("chosen root must have api::Task source");
        assert!(
            !source.messages.is_empty(),
            "chosen root must have non-empty messages",
        );
    }
}

#[test]
fn cli_agent_transcript_vehicle_is_excluded_from_navigation() {
    let conversation = AIConversation::new(false, true);

    assert!(conversation.should_exclude_from_navigation());
}

#[test]
fn restored_conversation_defaults_unknown_persisted_autoexecute_override() {
    let _flag = FeatureFlag::RememberFastForwardState.override_enabled(true);
    let conversation_data: AgentConversationData = serde_json::from_str(
        r#"{"server_conversation_token":null,"autoexecute_override":"UnexpectedValue"}"#,
    )
    .unwrap();

    let conversation = restored_conversation(Some(conversation_data));

    assert_eq!(
        conversation.autoexecute_override(),
        AIConversationAutoexecuteMode::RespectUserSettings
    );
}

#[test]
fn restored_conversation_uses_persisted_autoexecute_override_when_enabled() {
    let _flag = FeatureFlag::RememberFastForwardState.override_enabled(true);
    let conversation_data: AgentConversationData = serde_json::from_str(
        r#"{"server_conversation_token":null,"autoexecute_override":"RunToCompletion"}"#,
    )
    .unwrap();

    let conversation = restored_conversation(Some(conversation_data));

    assert_eq!(
        conversation.autoexecute_override(),
        AIConversationAutoexecuteMode::RunToCompletion
    );
}

#[test]
fn restored_conversation_ignores_persisted_autoexecute_override_when_disabled() {
    let _flag = FeatureFlag::RememberFastForwardState.override_enabled(false);
    let conversation_data: AgentConversationData = serde_json::from_str(
        r#"{"server_conversation_token":null,"autoexecute_override":"RunToCompletion"}"#,
    )
    .unwrap();

    let conversation = restored_conversation(Some(conversation_data));

    assert_eq!(
        conversation.autoexecute_override(),
        AIConversationAutoexecuteMode::RespectUserSettings
    );
}

#[test]
fn restored_conversation_loads_valid_byop_repair_sidecar() {
    let record = RepairRecord::new(
        RepairSource::ForkedHistory,
        ToolCallKey::new("root-task", "assistant-1", "call-1"),
    );
    let sidecar_json = serde_json::to_string(&RepairState::new(vec![record.clone()])).unwrap();
    let conversation_data: AgentConversationData = serde_json::from_value(serde_json::json!({
        "server_conversation_token": null,
        "byop_repair_state_json": sidecar_json,
    }))
    .unwrap();

    let conversation = restored_conversation(Some(conversation_data));

    assert_eq!(
        conversation.byop_repair_state,
        RepairStateStatus::Valid(RepairState::new(vec![record]))
    );
}

#[test]
fn restored_conversation_preserves_invalid_byop_repair_sidecar() {
    let sidecar_json = "{not valid json".to_string();
    let conversation_data: AgentConversationData = serde_json::from_value(serde_json::json!({
        "server_conversation_token": null,
        "byop_repair_state_json": sidecar_json,
    }))
    .unwrap();

    let conversation = restored_conversation(Some(conversation_data));

    assert!(matches!(
        conversation.byop_repair_state,
        RepairStateStatus::Invalid(InvalidRepairState {
            error_category: RepairStateLoadError::InvalidJson,
            ..
        })
    ));
    assert_eq!(
        conversation.byop_repair_state.to_sidecar_json().as_deref(),
        Some("{not valid json")
    );
}

#[test]
fn restored_conversation_does_not_infer_legacy_repair_for_unexplained_gap() {
    let conversation = AIConversation::new_restored(
        AIConversationId::new(),
        vec![api::Task {
            id: "root-task".to_string(),
            messages: vec![tool_call_message("assistant-1", "call-1")],
            dependencies: None,
            description: String::new(),
            summary: String::new(),
            server_data: String::new(),
        }],
        None,
    )
    .unwrap();

    assert_eq!(conversation.byop_repair_state, RepairStateStatus::default());
}

#[test]
fn byop_repair_sidecar_survives_serialization_after_fork_token_cleared() {
    let record = RepairRecord::new(
        RepairSource::ForkedHistory,
        ToolCallKey::new("root-task", "assistant-1", "call-1"),
    );
    let sidecar_json = serde_json::to_string(&RepairState::new(vec![record.clone()])).unwrap();
    let conversation_data: AgentConversationData = serde_json::from_value(serde_json::json!({
        "server_conversation_token": null,
        "forked_from_server_conversation_token": "source-token",
        "byop_repair_state_json": sidecar_json,
    }))
    .unwrap();
    let mut conversation = restored_conversation(Some(conversation_data));

    conversation.clear_forked_from_server_conversation_token();

    let ModelEvent::UpdateMultiAgentConversation {
        conversation_data, ..
    } = conversation.updated_conversation_state_event()
    else {
        panic!("expected conversation update event");
    };
    assert_eq!(
        conversation_data.forked_from_server_conversation_token,
        None
    );
    assert_eq!(
        RepairStateStatus::from_sidecar_json(conversation_data.byop_repair_state_json),
        RepairStateStatus::Valid(RepairState::new(vec![record]))
    );
}

#[allow(deprecated)]
#[test]
fn test_cli_subagent_serialized_block_preserves_block_id_and_metadata() {
    let cli_task_id = TaskId::new("cli-task-1".to_string());
    let conversation = AIConversation::new_restored(
        AIConversationId::new(),
        vec![
            api::Task {
                id: "root-task".to_string(),
                messages: vec![
                    tool_call_message_with_tool("tool-call-1", "call-1", run_shell_command_tool()),
                    tool_call_result_message_with_result(
                        "tool-result-1",
                        "call-1",
                        api::message::tool_call_result::Result::RunShellCommand(
                            api::RunShellCommandResult {
                                command: "echo hi".to_string(),
                                output: String::new(),
                                exit_code: 0,
                                result: Some(
                                    api::run_shell_command_result::Result::CommandFinished(
                                        api::ShellCommandFinished {
                                            start_ts: None,
                                            finish_ts: None,
                                            command_id: "cli-block-1".to_string(),
                                            output: "hi".to_string(),
                                            exit_code: 0,
                                        },
                                    ),
                                ),
                            },
                        ),
                    ),
                    tool_call_message_with_tool(
                        "subagent-call-1",
                        "subagent-call-1",
                        cli_subagent_tool(&String::from(cli_task_id.clone()), "cli-block-1"),
                    ),
                ],
                dependencies: None,
                description: String::new(),
                summary: String::new(),
                server_data: String::new(),
            },
            api::Task {
                id: String::from(cli_task_id.clone()),
                messages: vec![],
                dependencies: Some(api::task::Dependencies {
                    parent_task_id: "root-task".to_string(),
                }),
                description: String::new(),
                summary: String::new(),
                server_data: String::new(),
            },
        ],
        None,
    )
    .unwrap();

    let blocks = conversation.to_serialized_blocklist_items();
    let SerializedBlockListItem::Command { block } = &blocks[0];
    assert_eq!(block.id, BlockId::from("cli-block-1".to_string()));

    let metadata = block
        .ai_metadata
        .as_ref()
        .and_then(|json| serde_json::from_str::<Option<SerializedAIMetadata>>(json).ok())
        .flatten()
        .expect("CLI subagent command block should serialize AI metadata");
    let agent_metadata: AgentInteractionMetadata = metadata.into();
    assert_eq!(agent_metadata.conversation_id(), &conversation.id());
    assert_eq!(agent_metadata.subagent_task_id(), Some(&cli_task_id));
    assert!(agent_metadata.long_running_control_state().is_some());
    assert!(!agent_metadata.should_hide_block());
}

#[allow(deprecated)]
#[test]
fn test_cli_subagent_serialized_block_prefers_persisted_snapshot_output() {
    let conversation_id = AIConversationId::new();
    let cli_task_id = TaskId::new("cli-task-snapshot".to_string());
    let block_id = BlockId::from("cli-block-snapshot".to_string());
    let snapshot_output =
        b"* Documentation: https://help.ubuntu.com\r\nCONTAINER ID   IMAGE\r\nanalyzer-runtime\r\n";
    let mut conversation_data = empty_agent_conversation_data_for_test();
    conversation_data.cli_subagent_block_snapshots_json =
        Some(cli_subagent_snapshot_json_for_test(
            conversation_id,
            &cli_task_id,
            &block_id,
            b"ssh jump",
            snapshot_output,
        ));

    let conversation = AIConversation::new_restored(
        conversation_id,
        vec![
            api::Task {
                id: "root-task".to_string(),
                messages: vec![
                    tool_call_message_with_tool("tool-call-1", "call-1", run_shell_command_tool()),
                    tool_call_result_message_with_result(
                        "tool-result-1",
                        "call-1",
                        api::message::tool_call_result::Result::RunShellCommand(
                            api::RunShellCommandResult {
                                command: "ssh jump".to_string(),
                                output: String::new(),
                                exit_code: 0,
                                result: Some(
                                    api::run_shell_command_result::Result::CommandFinished(
                                        api::ShellCommandFinished {
                                            start_ts: None,
                                            finish_ts: None,
                                            command_id: String::from(block_id.clone()),
                                            output: "truncated task output".to_string(),
                                            exit_code: 0,
                                        },
                                    ),
                                ),
                            },
                        ),
                    ),
                    tool_call_message_with_tool(
                        "subagent-call-1",
                        "subagent-call-1",
                        cli_subagent_tool(&String::from(cli_task_id.clone()), block_id.as_str()),
                    ),
                ],
                dependencies: None,
                description: String::new(),
                summary: String::new(),
                server_data: String::new(),
            },
            api::Task {
                id: String::from(cli_task_id.clone()),
                messages: vec![],
                dependencies: Some(api::task::Dependencies {
                    parent_task_id: "root-task".to_string(),
                }),
                description: String::new(),
                summary: String::new(),
                server_data: String::new(),
            },
        ],
        Some(conversation_data),
    )
    .unwrap();

    let blocks = conversation.to_serialized_blocklist_items();
    let SerializedBlockListItem::Command { block } = &blocks[0];
    assert_eq!(block.id, block_id);
    let output = String::from_utf8_lossy(&block.stylized_output);
    assert!(
        output.contains("analyzer-runtime"),
        "restored block should use the persisted SSH terminal snapshot: {output}"
    );
    assert!(
        !output.contains("truncated task output"),
        "task-message output must not overwrite the richer terminal snapshot"
    );
}

#[allow(deprecated)]
#[test]
fn test_cli_subagent_serialized_block_restores_snapshot_without_command_result() {
    let conversation_id = AIConversationId::new();
    let cli_task_id = TaskId::new("cli-task-snapshot-only".to_string());
    let block_id = BlockId::from("cli-block-snapshot-only".to_string());
    let mut conversation_data = empty_agent_conversation_data_for_test();
    conversation_data.cli_subagent_block_snapshots_json =
        Some(cli_subagent_snapshot_json_for_test(
            conversation_id,
            &cli_task_id,
            &block_id,
            b"ssh jump",
            b"Last login: Wed Jul 1\r\ndocker ps -a\r\n",
        ));

    let conversation = AIConversation::new_restored(
        conversation_id,
        vec![
            api::Task {
                id: "root-task".to_string(),
                messages: vec![tool_call_message_with_tool(
                    "subagent-call-1",
                    "subagent-call-1",
                    cli_subagent_tool(&String::from(cli_task_id.clone()), block_id.as_str()),
                )],
                dependencies: None,
                description: String::new(),
                summary: String::new(),
                server_data: String::new(),
            },
            api::Task {
                id: String::from(cli_task_id.clone()),
                messages: vec![],
                dependencies: Some(api::task::Dependencies {
                    parent_task_id: "root-task".to_string(),
                }),
                description: String::new(),
                summary: String::new(),
                server_data: String::new(),
            },
        ],
        Some(conversation_data),
    )
    .unwrap();

    let blocks = conversation.to_serialized_blocklist_items();
    assert_eq!(blocks.len(), 1);
    let SerializedBlockListItem::Command { block } = &blocks[0];
    assert_eq!(block.id, block_id);
    assert!(
        String::from_utf8_lossy(&block.stylized_output).contains("docker ps -a"),
        "snapshot-only CLI subagent history should still restore terminal output"
    );
}

#[allow(deprecated)]
#[test]
fn test_cli_subagent_serialized_block_ignores_later_attachment_and_context_blocks() {
    let cli_task_id = TaskId::new("cli-task-attachment-context".to_string());
    let conversation = AIConversation::new_restored(
        AIConversationId::new(),
        vec![
            api::Task {
                id: "root-task".to_string(),
                messages: vec![
                    tool_call_message_with_tool("tool-call-1", "call-1", run_shell_command_tool()),
                    tool_call_result_message_with_result(
                        "tool-result-1",
                        "call-1",
                        api::message::tool_call_result::Result::RunShellCommand(
                            api::RunShellCommandResult {
                                command: "echo hi".to_string(),
                                output: String::new(),
                                exit_code: 0,
                                result: Some(
                                    api::run_shell_command_result::Result::CommandFinished(
                                        api::ShellCommandFinished {
                                            start_ts: None,
                                            finish_ts: None,
                                            command_id: "cli-block-1".to_string(),
                                            output: "hi".to_string(),
                                            exit_code: 0,
                                        },
                                    ),
                                ),
                            },
                        ),
                    ),
                    user_query_message_with_shell_context(
                        "user-with-shell-context",
                        "request-attachment-context",
                        "show recent shell state",
                        "cat attachment.txt",
                        "cat context.txt",
                    ),
                    tool_call_message_with_tool(
                        "subagent-call-1",
                        "subagent-call-1",
                        cli_subagent_tool(&String::from(cli_task_id.clone()), "cli-block-1"),
                    ),
                ],
                dependencies: None,
                description: String::new(),
                summary: String::new(),
                server_data: String::new(),
            },
            api::Task {
                id: String::from(cli_task_id.clone()),
                messages: vec![],
                dependencies: Some(api::task::Dependencies {
                    parent_task_id: "root-task".to_string(),
                }),
                description: String::new(),
                summary: String::new(),
                server_data: String::new(),
            },
        ],
        None,
    )
    .unwrap();

    let blocks = conversation.to_serialized_blocklist_items();
    assert_eq!(blocks.len(), 3);

    let SerializedBlockListItem::Command {
        block: run_shell_block,
    } = &blocks[0];
    assert_eq!(run_shell_block.id, BlockId::from("cli-block-1".to_string()));
    let run_shell_metadata = run_shell_block
        .ai_metadata
        .as_ref()
        .and_then(|json| serde_json::from_str::<Option<SerializedAIMetadata>>(json).ok())
        .flatten()
        .expect("CLI subagent metadata should stay on the RunShellCommand block");
    let run_shell_agent_metadata: AgentInteractionMetadata = run_shell_metadata.into();
    assert_eq!(
        run_shell_agent_metadata.subagent_task_id(),
        Some(&cli_task_id)
    );

    let SerializedBlockListItem::Command {
        block: attachment_block,
    } = &blocks[1];
    assert_eq!(
        String::from_utf8_lossy(&attachment_block.stylized_command),
        "cat attachment.txt"
    );
    assert!(attachment_block.ai_metadata.is_none());
    assert_ne!(
        attachment_block.id,
        BlockId::from("cli-block-1".to_string())
    );

    let SerializedBlockListItem::Command {
        block: context_block,
    } = &blocks[2];
    assert_eq!(
        String::from_utf8_lossy(&context_block.stylized_command),
        "cat context.txt"
    );
    assert!(context_block.ai_metadata.is_none());
    assert_ne!(context_block.id, BlockId::from("cli-block-1".to_string()));
}

#[allow(deprecated)]
#[test]
fn test_cli_subagent_serialized_block_uses_metadata_command_id_not_latest_command() {
    let cli_task_id = TaskId::new("cli-task-first-command".to_string());
    let conversation = AIConversation::new_restored(
        AIConversationId::new(),
        vec![
            api::Task {
                id: "root-task".to_string(),
                messages: vec![
                    tool_call_message_with_tool("tool-call-1", "call-1", run_shell_command_tool()),
                    tool_call_result_message_with_result(
                        "tool-result-1",
                        "call-1",
                        api::message::tool_call_result::Result::RunShellCommand(
                            api::RunShellCommandResult {
                                command: "echo hi".to_string(),
                                output: String::new(),
                                exit_code: 0,
                                result: Some(
                                    api::run_shell_command_result::Result::CommandFinished(
                                        api::ShellCommandFinished {
                                            start_ts: None,
                                            finish_ts: None,
                                            command_id: "cli-block-1".to_string(),
                                            output: "first".to_string(),
                                            exit_code: 0,
                                        },
                                    ),
                                ),
                            },
                        ),
                    ),
                    tool_call_message_with_tool("tool-call-2", "call-2", run_shell_command_tool()),
                    tool_call_result_message_with_result(
                        "tool-result-2",
                        "call-2",
                        api::message::tool_call_result::Result::RunShellCommand(
                            api::RunShellCommandResult {
                                command: "echo hi".to_string(),
                                output: String::new(),
                                exit_code: 0,
                                result: Some(
                                    api::run_shell_command_result::Result::CommandFinished(
                                        api::ShellCommandFinished {
                                            start_ts: None,
                                            finish_ts: None,
                                            command_id: "other-block".to_string(),
                                            output: "second".to_string(),
                                            exit_code: 0,
                                        },
                                    ),
                                ),
                            },
                        ),
                    ),
                    tool_call_message_with_tool(
                        "subagent-call-1",
                        "subagent-call-1",
                        cli_subagent_tool(&String::from(cli_task_id.clone()), "cli-block-1"),
                    ),
                ],
                dependencies: None,
                description: String::new(),
                summary: String::new(),
                server_data: String::new(),
            },
            api::Task {
                id: String::from(cli_task_id.clone()),
                messages: vec![],
                dependencies: Some(api::task::Dependencies {
                    parent_task_id: "root-task".to_string(),
                }),
                description: String::new(),
                summary: String::new(),
                server_data: String::new(),
            },
        ],
        None,
    )
    .unwrap();

    let blocks = conversation.to_serialized_blocklist_items();
    assert_eq!(blocks.len(), 2);

    let SerializedBlockListItem::Command { block: first_block } = &blocks[0];
    assert_eq!(first_block.id, BlockId::from("cli-block-1".to_string()));
    let first_metadata = first_block
        .ai_metadata
        .as_ref()
        .and_then(|json| serde_json::from_str::<Option<SerializedAIMetadata>>(json).ok())
        .flatten()
        .expect("CLI subagent metadata should attach to the referenced command block");
    let first_agent_metadata: AgentInteractionMetadata = first_metadata.into();
    assert_eq!(first_agent_metadata.subagent_task_id(), Some(&cli_task_id));
    assert_eq!(
        String::from_utf8_lossy(&first_block.stylized_output),
        "first"
    );

    let SerializedBlockListItem::Command {
        block: second_block,
    } = &blocks[1];
    assert_ne!(second_block.id, BlockId::from("cli-block-1".to_string()));
    assert_eq!(
        String::from_utf8_lossy(&second_block.stylized_output),
        "second"
    );
    let second_metadata = second_block
        .ai_metadata
        .as_ref()
        .and_then(|json| serde_json::from_str::<Option<SerializedAIMetadata>>(json).ok())
        .flatten()
        .expect("second command should keep requested-command metadata");
    let second_agent_metadata: AgentInteractionMetadata = second_metadata.into();
    assert_eq!(second_agent_metadata.subagent_task_id(), None);
}

#[test]
fn fork_artifacts_adds_file_artifacts_to_conversation() {
    let proto_artifact = api::message::artifact_event::ConversationArtifact {
        artifact: Some(
            api::message::artifact_event::conversation_artifact::Artifact::File(
                api::message::artifact_event::FileArtifact {
                    artifact_uid: "artifact-file-1".to_string(),
                    filepath: "outputs/report.txt".to_string(),
                    mime_type: "text/plain".to_string(),
                    size_bytes: 42,
                    description: "Daily summary".to_string(),
                },
            ),
        ),
    };

    assert_eq!(
        artifact_from_fork_proto(&proto_artifact),
        Some(Artifact::File {
            artifact_uid: "artifact-file-1".to_string(),
            filepath: "outputs/report.txt".to_string(),
            filename: "report.txt".to_string(),
            mime_type: "text/plain".to_string(),
            description: Some("Daily summary".to_string()),
            size_bytes: Some(42),
        })
    );
}

#[test]
fn waiting_for_events_display_label_is_waiting() {
    assert_eq!(
        format!("{}", ConversationStatus::WaitingForEvents),
        "Waiting"
    );
}

/// `is_done` returns true only for `Success | Error | Cancelled`;
/// `WaitingForEvents` and `Blocked` are not done because the run can still
/// resume on its own.
#[test]
fn is_done_only_includes_success_error_cancelled() {
    assert!(ConversationStatus::Success.is_done());
    assert!(ConversationStatus::Error.is_done());
    assert!(ConversationStatus::Cancelled.is_done());

    assert!(!ConversationStatus::InProgress.is_done());
    assert!(
        !ConversationStatus::Blocked {
            blocked_action: "approve".to_string()
        }
        .is_done()
    );
    assert!(!ConversationStatus::WaitingForEvents.is_done());
}

/// `is_waiting_for_events` is true only for the new variant.
#[test]
fn is_waiting_for_events_returns_true_only_for_waiting_for_events_variant() {
    assert!(ConversationStatus::WaitingForEvents.is_waiting_for_events());

    assert!(!ConversationStatus::InProgress.is_waiting_for_events());
    assert!(!ConversationStatus::Success.is_waiting_for_events());
    assert!(!ConversationStatus::Error.is_waiting_for_events());
    assert!(!ConversationStatus::Cancelled.is_waiting_for_events());
    assert!(
        !ConversationStatus::Blocked {
            blocked_action: "approve".to_string()
        }
        .is_waiting_for_events()
    );
}

/// A conversation that was yielded via `wait_for_events` at shutdown
/// restores as whatever `derive_status_from_root_task` returns (Success
/// for a cleanly-streamed last exchange). The unresolved tool call stays
/// in the transcript as an orphan; the next outbound request triggers
/// the server's existing supersede mechanism to synthesize the matching
/// `Cancel`. The waiting state itself is not durable across restart.
#[test]
fn restored_conversation_does_not_re_enter_waiting_for_events() {
    let conversation_data: AgentConversationData =
        serde_json::from_str(r#"{"server_conversation_token":null}"#).unwrap();

    let conversation = restored_conversation(Some(conversation_data));

    assert_eq!(conversation.status(), &ConversationStatus::Success);
}
