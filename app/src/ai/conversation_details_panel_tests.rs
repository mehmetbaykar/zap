//! Tests for `ConversationDetailsData` construction.
//!
//! Zap adaptations from upstream:
//! - `test_from_conversation_prefers_server_creator_profile` is dropped: it exercises
//!   `AIConversation::set_server_metadata`/`ServerAIConversationMetadata { metadata, creator,
//!   permissions, .. }`, none of which exist in this fork's (de-clouded) conversation/server
//!   metadata types.
//! - `test_oz_run_url_present_for_task_and_absent_for_conversation` is dropped: there is no
//!   `ConversationDetailsPanel::oz_run_url` in this fork (the "Open in Oz" link surface requires
//!   `ChannelState::oz_root_url()`, which does not exist here — there is no Oz web app to link
//!   to).
//! - `test_from_conversation_populates_local_conversation_fields` is ported together with its
//!   upstream test-local helpers (`create_restored_conversation`, `create_message_with_directory`,
//!   `create_agent_output_message`); their `warp_multi_agent_api` message shapes are the ones
//!   `terminal/conversation_restoration_tests.rs` already builds against this fork. The persisted
//!   `AgentConversationData` is `AgentConversationData::default()` (all `None`/`false`, the same
//!   values as upstream's explicit literal).
//! - The two `test_from_task_includes_linked_directory_when_*` tests are dropped: they cover the
//!   task-backed (cloud `AmbientAgentTask`) mode resolving a linked conversation's directory,
//!   which this fork's `from_task` does not do (it always sets `directory: None`).
//! - `create_test_task` drops the `run_time: Some("PT1S".parse().unwrap())` field: this fork's
//!   `AmbientAgentTask` has no `run_time` field (it's `AmbientAgentTask::run_time()`, a method
//!   computed from `started_at`/`updated_at`, not stored data).

use std::collections::HashMap;

use chrono::Utc;
use persistence::model::AgentConversationData;
use warp_cli::agent::Harness;
use warp_multi_agent_api as api;
use warpui::{App, EntityId, SingletonEntity};

use super::{ConversationDetailsData, PanelMode};
use crate::ai::agent::conversation::{AIConversation, AIConversationId};
use crate::ai::ambient_agents::task::{AgentConfigSnapshot, HarnessConfig, TaskPrincipalInfo};
use crate::ai::ambient_agents::{AmbientAgentTask, AmbientAgentTaskState};
use crate::ai::blocklist::history_model::BlocklistAIHistoryModel;

fn create_test_task(task_id: &str) -> AmbientAgentTask {
    let now = Utc::now();
    AmbientAgentTask {
        task_id: task_id.parse().unwrap(),
        parent_run_id: None,
        title: "Task".to_string(),
        state: AmbientAgentTaskState::Succeeded,
        prompt: "test".to_string(),
        created_at: now,
        started_at: None,
        updated_at: now,
        status_message: None,
        source: None,
        session_id: None,
        session_link: None,
        creator: Some(TaskPrincipalInfo {
            creator_type: "USER".to_string(),
            uid: "user-1".to_string(),
            display_name: Some("User 1".to_string()),
        }),
        executor: None,
        conversation_id: None,
        request_usage: None,
        agent_config_snapshot: None,
        artifacts: vec![],
        is_sandbox_running: false,
        last_event_sequence: None,
        children: vec![],
    }
}

fn create_message_with_directory(id: &str, task_id: &str, directory: &str) -> api::Message {
    api::Message {
        fetched_memories: vec![],
        id: id.to_string(),
        task_id: task_id.to_string(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::UserQuery(api::message::UserQuery {
            query: "test query".to_string(),
            context: Some(api::InputContext {
                directory: Some(api::input_context::Directory {
                    pwd: directory.to_string(),
                    home: String::new(),
                    pwd_file_symbols_indexed: false,
                }),
                ..Default::default()
            }),
            referenced_attachments: HashMap::new(),
            mode: None,
            intended_agent: Default::default(),
            origin: None,
            author: None,
            source_message: None,
        })),
        request_id: "request-1".to_string(),
        timestamp: None,
    }
}

fn create_agent_output_message(id: &str, task_id: &str) -> api::Message {
    api::Message {
        fetched_memories: vec![],
        id: id.to_string(),
        task_id: task_id.to_string(),
        server_message_data: String::new(),
        citations: vec![],
        message: Some(api::message::Message::AgentOutput(
            api::message::AgentOutput {
                text: "done".to_string(),
            },
        )),
        request_id: "request-1".to_string(),
        timestamp: None,
    }
}

fn create_restored_conversation(
    conversation_id: AIConversationId,
    root_task_id: &str,
    directory: &str,
    conversation_data: AgentConversationData,
) -> AIConversation {
    let task = api::Task {
        id: root_task_id.to_string(),
        messages: vec![
            create_message_with_directory("message-1", root_task_id, directory),
            create_agent_output_message("message-2", root_task_id),
        ],
        dependencies: None,
        description: String::new(),
        summary: String::new(),
        server_data: String::new(),
    };

    AIConversation::new_restored(conversation_id, vec![task], Some(conversation_data))
        .expect("restored conversation should build")
}

#[test]
fn test_from_conversation_metadata_passes_harness_through() {
    for harness in [
        None,
        Some(Harness::Oz),
        Some(Harness::Claude),
        Some(Harness::Gemini),
        Some(Harness::Unknown),
    ] {
        let data = ConversationDetailsData::from_conversation_metadata(
            AIConversationId::new(),
            "Title".to_string(),
            None,
            Utc::now().with_timezone(&chrono::Local),
            None,
            None,
            None,
            vec![],
            None,
            None,
            None,
            None,
            harness,
        );
        assert_eq!(
            data.harness, harness,
            "harness {harness:?} should pass through"
        );
    }
}

#[test]
fn test_from_task_resolves_harness() {
    App::test((), |mut app| async move {
        let _history_model =
            app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], vec![], &[]));

        // Base task has `agent_config_snapshot: None`; cloning lets us mutate per case.
        let base_task = create_test_task("550e8400-e29b-41d4-a716-000000004020");

        app.update(|ctx| {
            // No snapshot -> harness unknown.
            let data = ConversationDetailsData::from_task(&base_task, None, None, ctx);
            assert_eq!(data.harness, None);

            // Snapshot without an explicit harness -> default to Oz.
            let mut task = base_task.clone();
            task.agent_config_snapshot = Some(AgentConfigSnapshot::default());
            let data = ConversationDetailsData::from_task(&task, None, None, ctx);
            assert_eq!(data.harness, Some(Harness::Oz));

            // Snapshot with explicit harness_type.
            for harness in [
                Harness::Oz,
                Harness::Claude,
                Harness::Gemini,
                Harness::Unknown,
            ] {
                let mut task = base_task.clone();
                task.agent_config_snapshot = Some(AgentConfigSnapshot {
                    harness: Some(HarnessConfig::from_harness_type(harness)),
                    ..Default::default()
                });
                let data = ConversationDetailsData::from_task(&task, None, None, ctx);
                assert_eq!(data.harness, Some(harness), "harness {harness:?}");
            }
        });
    });
}

#[test]
fn test_from_task_populates_executor() {
    App::test((), |mut app| async move {
        let _history_model =
            app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], vec![], &[]));
        let mut task = create_test_task("550e8400-e29b-41d4-a716-000000004030");
        task.executor = Some(TaskPrincipalInfo {
            creator_type: "service_account".to_string(),
            uid: "agent-uid".to_string(),
            display_name: Some("Deploy Agent".to_string()),
        });

        app.update(|ctx| {
            let data = ConversationDetailsData::from_task(&task, None, None, ctx);
            assert_eq!(
                data.executor
                    .as_ref()
                    .map(|executor| executor.display_name.as_str()),
                Some("Deploy Agent")
            );
        });
    });
}

#[test]
fn test_from_conversation_populates_local_conversation_fields() {
    // Locks in that `ConversationDetailsData::from_conversation` works on native
    // and surfaces the conversation-derived fields the pane-level conversation
    // details panel renders for local agent conversations.
    App::test((), |mut app| async move {
        let history_model =
            app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], vec![], &[]));

        let conversation_id = AIConversationId::new();
        let directory = "/tmp/local-conversation-directory";
        let conversation = create_restored_conversation(
            conversation_id,
            "root-task",
            directory,
            AgentConversationData::default(),
        );

        history_model.update(&mut app, |model, ctx| {
            model.restore_conversations(EntityId::new(), vec![conversation], ctx);
        });

        app.update(|ctx| {
            let conversation = BlocklistAIHistoryModel::as_ref(ctx)
                .conversation(&conversation_id)
                .expect("conversation should be present");
            let data = ConversationDetailsData::from_conversation(conversation, ctx);

            // Mode should be Conversation with the working directory and no server-side
            // conversation id (since this conversation was restored without a server token).
            match &data.mode {
                PanelMode::Conversation {
                    directory: panel_directory,
                    server_conversation_id,
                    ai_conversation_id,
                    status,
                } => {
                    assert_eq!(panel_directory.as_deref(), Some(directory));
                    assert!(server_conversation_id.is_none());
                    // `from_conversation` does not have access to the in-memory
                    // AIConversationId; that field is populated only by the
                    // management view path (`from_conversation_metadata`).
                    assert!(ai_conversation_id.is_none());
                    assert!(status.is_some());
                }
                PanelMode::Task { .. } => {
                    panic!("expected Conversation mode for a local conversation")
                }
            }

            assert_eq!(data.title, "test query");
            assert_eq!(data.source_prompt.as_deref(), Some("test query"));
            assert!(data.credits.is_some());
        });
    });
}

#[test]
fn test_from_task_id_carries_error_message() {
    let task_id: crate::ai::ambient_agents::AmbientAgentTaskId =
        "550e8400-e29b-41d4-a716-000000004040".parse().unwrap();
    let data = ConversationDetailsData::from_task_id(task_id, Some("not found".to_string()));
    match data.mode {
        PanelMode::Task {
            task_id: Some(id),
            error_message: Some(message),
            ..
        } => {
            assert_eq!(id, task_id);
            assert_eq!(message, "not found");
        }
        other => panic!("expected Task mode with an error message, got {other:?}"),
    }
}
