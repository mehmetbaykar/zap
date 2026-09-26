use ai::agent::action::{RunAgentsAgentRunConfig, RunAgentsExecutionMode, RunAgentsRequest};
use ai::agent::action_result::RunAgentsAgentOutcomeKind;
use async_channel::unbounded;
use warp_core::features::FeatureFlag;
use warpui::{App, EntityId};

use super::*;
use crate::LaunchMode;
use crate::ai::agent::task::TaskId;
use crate::ai::agent::{AIAgentAction, AIAgentActionId, StartAgentExecutionMode};
use crate::ai::blocklist::{BlocklistAIHistoryModel, BlocklistAIPermissions};
use crate::ai::execution_profiles::RunAgentsPermission;
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::mcp::templatable_manager::TemplatableMCPServerManager;
use crate::auth::AuthStateProvider;
use crate::cloud_object::model::persistence::ObjectStoreModel;
use crate::cloud_object::update_manager::UpdateManager;
use crate::network::NetworkStatus;
use crate::terminal::model::session::{BootstrapSessionType, SessionId, SessionInfo, Sessions};
use crate::terminal::model_events::ModelEventDispatcher;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::workspaces::user_workspaces::UserWorkspaces;

#[test]
fn compose_child_prompt_joins_non_empty_parts() {
    assert_eq!(
        compose_run_agents_child_prompt("shared", "specialized"),
        "shared\n\nspecialized"
    );
}

#[test]
fn compose_child_prompt_does_not_add_blank_separators() {
    assert_eq!(compose_run_agents_child_prompt("shared", "  "), "shared");
    assert_eq!(
        compose_run_agents_child_prompt("", "specialized"),
        "specialized"
    );
    assert_eq!(compose_run_agents_child_prompt("", ""), "");
}

#[test]
fn local_codex_batch_maps_to_local_codex_children() {
    let _local_codex = FeatureFlag::LocalClaudeCodexChildHarnesses.override_enabled(true);
    assert_eq!(
        run_agents_to_start_agent_mode(&RunAgentsExecutionMode::Local, "codex", "provider/model"),
        Ok(StartAgentExecutionMode::Local {
            harness_type: Some("codex".to_string()),
            model_id: Some("provider/model".to_string()),
        })
    );
}

#[test]
fn local_oz_batch_maps_to_embedded_children() {
    assert_eq!(
        run_agents_to_start_agent_mode(&RunAgentsExecutionMode::Local, "oz", ""),
        Ok(StartAgentExecutionMode::local_with_defaults())
    );
}

#[test]
fn unsupported_local_harness_is_rejected() {
    assert_eq!(
        run_agents_to_start_agent_mode(&RunAgentsExecutionMode::Local, "future-cli", ""),
        Err("Unsupported local child harness 'future-cli'.".to_string())
    );
}

#[test]
fn child_outcomes_preserve_request_order() {
    let configs = vec![
        RunAgentsAgentRunConfig {
            name: "first".to_string(),
            prompt: String::new(),
            title: String::new(),
            model_id: String::new(),
        },
        RunAgentsAgentRunConfig {
            name: "second".to_string(),
            prompt: String::new(),
            title: String::new(),
            model_id: String::new(),
        },
    ];
    let outcomes = build_agent_outcomes(
        &configs,
        vec![
            RunAgentsAgentOutcomeKind::Launched {
                agent_id: "one".to_string(),
            },
            RunAgentsAgentOutcomeKind::Failed {
                error: "failed".to_string(),
            },
        ],
        "",
    );

    assert_eq!(outcomes[0].name, "first");
    assert_eq!(outcomes[1].name, "second");
    assert_eq!(
        outcomes[0].kind,
        RunAgentsAgentOutcomeKind::Launched {
            agent_id: "one".to_string(),
        }
    );
    assert_eq!(
        outcomes[1].kind,
        RunAgentsAgentOutcomeKind::Failed {
            error: "failed".to_string(),
        }
    );
}

// ---------------------------------------------------------------------------
// Approval gate.
//
// These cover the question the compile gates cannot answer: does a `run_agents`
// batch actually stop for the user? `should_autoexecute` returning false is what
// routes the action to the confirmation card in `execute.rs`, so each case below
// is the difference between a child agent launching unattended and the user
// being asked first.
// ---------------------------------------------------------------------------

fn build_run_agents_action(action_id: &str, plan_id: &str) -> AIAgentAction {
    AIAgentAction {
        id: AIAgentActionId::from(action_id.to_string()),
        action: AIAgentActionType::RunAgents(RunAgentsRequest {
            summary: "batch".to_string(),
            base_prompt: "shared".to_string(),
            skills: Vec::new(),
            model_id: String::new(),
            harness_type: String::new(),
            execution_mode: RunAgentsExecutionMode::Local,
            agent_run_configs: vec![RunAgentsAgentRunConfig {
                name: "first".to_string(),
                prompt: "do the thing".to_string(),
                title: "first".to_string(),
                model_id: String::new(),
            }],
            plan_id: plan_id.to_string(),
            harness_auth_secret_name: None,
        }),
        task_id: TaskId::new(format!("task-{action_id}")),
        requires_result: true,
    }
}

/// Builds the active session the executor consults, local or Warpified remote.
fn add_active_session(
    app: &mut App,
    session_type: BootstrapSessionType,
) -> ModelHandle<ActiveSession> {
    let session_id = SessionId::from(42);
    let sessions = app.add_model(|_| Sessions::new_for_test());
    sessions.update(app, |sessions, _ctx| {
        sessions.register_session_for_test(
            SessionInfo::new_for_test()
                .with_id(session_id)
                .with_session_type(session_type),
        );
    });
    let (_model_events_tx, model_events_rx) = unbounded();
    let model_event_dispatcher =
        app.add_model(|ctx| ModelEventDispatcher::new(model_events_rx, sessions.clone(), ctx));
    model_event_dispatcher.update(app, |dispatcher, _ctx| {
        dispatcher.set_active_session_id(session_id);
    });
    app.add_model(|ctx| ActiveSession::new(sessions, model_event_dispatcher, ctx))
}

/// Registers what `should_autoexecute` and execution preparation read, and returns an executor
/// plus a fresh conversation for a terminal whose active profile has `permission`.
fn add_test_executor(
    app: &mut App,
    permission: RunAgentsPermission,
    session_type: BootstrapSessionType,
) -> (ModelHandle<RunAgentsExecutor>, AIConversationId) {
    let terminal_view_id = EntityId::new();
    initialize_settings_for_tests(app);
    let history = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(UpdateManager::mock);
    app.add_singleton_model(ObjectStoreModel::mock);
    app.add_singleton_model(|_| TemplatableMCPServerManager::default());
    app.add_singleton_model(UserWorkspaces::default_mock);
    // `initialize_settings_for_tests` already registers AppExecutionMode as
    // ExecutionMode::App, which is not autonomous. That matters: an
    // autonomous mode short-circuits `should_autoexecute` to true and would
    // make every case below pass without exercising the permission at all.
    let profiles = app.add_singleton_model(|ctx| {
        AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
    });
    app.add_singleton_model(BlocklistAIPermissions::new);

    profiles.update(app, |profiles, ctx| {
        if let Some(profile_id) = profiles.create_profile(ctx) {
            profiles.set_run_agents(&profile_id, permission, ctx);
            profiles.set_active_profile(terminal_view_id, profile_id, ctx);
        }
    });

    let conversation_id = history.update(app, |history, ctx| {
        history.start_new_conversation(terminal_view_id, true, false, false, ctx)
    });
    let start_agent_executor = app.add_model(|_| StartAgentExecutor::new(terminal_view_id));
    let active_session = add_active_session(app, session_type);
    let executor = app.add_model(|_| {
        RunAgentsExecutor::new(start_agent_executor, active_session, terminal_view_id)
    });
    (executor, conversation_id)
}

/// Drives `should_autoexecute` under `permission` and asserts the outcome.
///
/// The assertion happens inside the `App::test` future because its closure must
/// be `'static`, so a result cannot be borrowed back out to the caller.
fn assert_autoexecute(permission: RunAgentsPermission, plan_id: &'static str, expected: bool) {
    App::test((), move |mut app| async move {
        let (executor, conversation_id) =
            add_test_executor(&mut app, permission, BootstrapSessionType::Local);
        let action = build_run_agents_action("run-agents", plan_id);

        let autoexecuted = executor.update(&mut app, |executor, ctx| {
            executor.should_autoexecute(
                ExecuteActionInput {
                    action: &action,
                    conversation_id,
                },
                ctx,
            )
        });

        assert_eq!(autoexecuted, expected);
    });
}

#[test]
fn always_ask_stops_for_the_confirmation_card() {
    assert_autoexecute(RunAgentsPermission::AlwaysAsk, "", false);
}

#[test]
fn never_allow_does_not_autoexecute() {
    assert_autoexecute(RunAgentsPermission::NeverAllow, "", false);
}

#[test]
fn always_allow_autoexecutes() {
    assert_autoexecute(RunAgentsPermission::AlwaysAllow, "", true);
}

#[test]
fn empty_plan_id_does_not_count_as_an_approved_plan() {
    // BYOP `run_agents` calls always carry an empty plan id. Before the guard in
    // `Conversation::orchestration_config_for_plan`, "" was an ordinary map key,
    // so a config stored under it would have been read as an approved plan and
    // shortcut `should_autoexecute` to true -- skipping the card for every
    // unplanned batch. Under AlwaysAsk an empty plan id must still ask.
    assert_autoexecute(RunAgentsPermission::AlwaysAsk, "", false);
}

/// Children launch in local panes, so a parent in an SSH session must be refused rather than
/// have its children run on this machine; the refusal skips the approval prompt.
#[test]
fn remote_session_refuses_without_prompting() {
    App::test((), |mut app| async move {
        let (executor, conversation_id) = add_test_executor(
            &mut app,
            RunAgentsPermission::AlwaysAsk,
            BootstrapSessionType::WarpifiedRemote,
        );
        let action = build_run_agents_action("run-agents", "");
        let AIAgentActionType::RunAgents(request) = &action.action else {
            unreachable!("built a run_agents action");
        };

        executor.update(&mut app, |executor, ctx| {
            assert!(executor.should_autoexecute(
                ExecuteActionInput {
                    action: &action,
                    conversation_id,
                },
                ctx,
            ));
            let reason = prepare_request_for_execution(
                &mut request.clone(),
                conversation_id,
                executor.terminal_view_id,
                &executor.active_session,
                &executor.launched_agents,
                ctx,
            );
            assert!(reason.is_some_and(|reason| reason.contains("SSH session")));
        });
    });
}
