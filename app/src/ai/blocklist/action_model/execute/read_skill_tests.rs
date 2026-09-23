use std::fs;
use std::io::Write;
use std::path::PathBuf;

use ai::agent::action_result::AnyFileContent;
use ai::skills::{ParsedSkill, SkillProvider, SkillReference, SkillScope, parse_skill};
use async_channel::unbounded;
use repo_metadata::RepoMetadataModel;
use repo_metadata::repositories::DetectedRepositories;
use repo_metadata::watcher::DirectoryWatcher;
use tempfile::TempDir;
use warp_core::HostId;
use warp_core::features::FeatureFlag;
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warp_util::remote_path::RemotePath;
use warp_util::standardized_path::StandardizedPath;
use warpui::{App, ModelHandle};
use watcher::HomeDirectoryWatcher;

use super::*;
use crate::ai::agent::task::TaskId;
use crate::ai::agent::{
    AIAgentAction, AIAgentActionId, AIAgentActionResultType, AIAgentActionType, ReadSkillRequest,
    ReadSkillResult,
};
use crate::ai::blocklist::action_model::AIConversationId;
use crate::ai::skills::{BundledSkillActivation, SkillManager};
use crate::terminal::model::session::active_session::ActiveSession;
use crate::terminal::model::session::{BootstrapSessionType, SessionId, SessionInfo, Sessions};
use crate::terminal::model_events::ModelEventDispatcher;
use crate::warp_managed_paths_watcher::WarpManagedPathsWatcher;

fn initialize_app(app: &mut App) {
    app.add_singleton_model(DirectoryWatcher::new);
    app.add_singleton_model(|_| DetectedRepositories::default());
    app.add_singleton_model(RepoMetadataModel::new);
    app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
    app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
    app.add_singleton_model(SkillManager::new);
}

fn add_test_read_skill_executor(app: &mut App) -> ModelHandle<ReadSkillExecutor> {
    let sessions = app.add_model(|_| Sessions::new_for_test());
    let (_model_events_tx, model_events_rx) = unbounded();
    let model_event_dispatcher =
        app.add_model(|ctx| ModelEventDispatcher::new(model_events_rx, sessions.clone(), ctx));
    let active_session = app
        .add_model(|ctx| ActiveSession::new(sessions.clone(), model_event_dispatcher.clone(), ctx));
    app.add_model(|_| ReadSkillExecutor::new(active_session))
}

/// Builds an executor whose active session is a Warpified remote session, connected to
/// `host_id` when it is `Some` and still waiting for its remote server otherwise.
fn add_remote_read_skill_executor(
    app: &mut App,
    host_id: Option<HostId>,
) -> ModelHandle<ReadSkillExecutor> {
    let session_id = SessionId::from(42);
    let sessions = app.add_model(|_| Sessions::new_for_test());
    sessions.update(app, |sessions, _ctx| {
        sessions.register_session_for_test(
            SessionInfo::new_for_test()
                .with_id(session_id)
                .with_session_type(BootstrapSessionType::WarpifiedRemote),
        );
    });
    if let Some(host_id) = host_id {
        let session = sessions
            .read(app, |sessions, _ctx| sessions.get(session_id))
            .unwrap();
        session.set_remote_host_id(Some(host_id));
    }

    let (_model_events_tx, model_events_rx) = unbounded();
    let model_event_dispatcher =
        app.add_model(|ctx| ModelEventDispatcher::new(model_events_rx, sessions.clone(), ctx));
    model_event_dispatcher.update(app, |dispatcher, _ctx| {
        dispatcher.set_active_session_id(session_id);
    });
    let active_session = app
        .add_model(|ctx| ActiveSession::new(sessions.clone(), model_event_dispatcher.clone(), ctx));
    app.add_model(|_| ReadSkillExecutor::new(active_session))
}

fn bundled_skill(name: &str) -> ParsedSkill {
    ParsedSkill {
        name: name.to_string(),
        description: format!("{name} bundled skill"),
        path: LocalOrRemotePath::Local(PathBuf::from(format!("/bundled/skills/{name}/SKILL.md"))),
        content: format!("# {name}"),
        line_range: None,
        provider: SkillProvider::Zap,
        scope: SkillScope::Bundled,
    }
}

fn remote_skill_path(host_id: &HostId, path: &str) -> LocalOrRemotePath {
    LocalOrRemotePath::Remote(RemotePath::new(
        host_id.clone(),
        StandardizedPath::try_new(path).unwrap(),
    ))
}

/// Builds a `ReadSkill` action for `skill`.
fn read_skill_action(skill: SkillReference) -> AIAgentAction {
    AIAgentAction {
        id: AIAgentActionId::from("test-action-id".to_string()),
        action: AIAgentActionType::ReadSkill(ReadSkillRequest { skill }),
        task_id: TaskId::new("test-task-id".to_string()),
        requires_result: false,
    }
}

/// Builds the reference the BYOP `read_skill` tool produces: the bare skill name packed into
/// the `Path` slot (see `agent_providers::tools::skill`).
fn byop_name_reference(name: &str) -> SkillReference {
    SkillReference::Path(LocalOrRemotePath::Local(PathBuf::from(name)))
}

/// Runs `action` synchronously and returns the read skill's `(file_name, content)`, or the
/// error message when the read failed.
fn execute_sync_read(
    app: &mut App,
    executor_handle: &ModelHandle<ReadSkillExecutor>,
    action: &AIAgentAction,
) -> Result<(String, AnyFileContent), String> {
    executor_handle.update(app, |executor, ctx| {
        let input = ExecuteActionInput {
            action,
            conversation_id: AIConversationId::new(),
        };
        let result: AnyActionExecution = executor.execute(input, ctx).into();
        match result {
            AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                ReadSkillResult::Success { content },
            )) => Ok((content.file_name, content.content)),
            AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                ReadSkillResult::Error(message),
            )) => Err(message),
            _ => panic!("read_skill should resolve synchronously"),
        }
    })
}

fn create_test_skill_file(dir: &TempDir, name: &str, description: &str) -> std::path::PathBuf {
    let skill_content = format!(
        r#"---
name: {}
description: {}
---

# {}

## Instructions
Test instructions for this skill.

## Examples
Example usage of the skill.
"#,
        name, description, name
    );

    let skill_dir = dir.path().join(format!(".claude/skills/{}", name));
    fs::create_dir_all(&skill_dir).unwrap();
    let skill_path = skill_dir.join("SKILL.md");
    let mut file = fs::File::create(&skill_path).unwrap();
    file.write_all(skill_content.as_bytes()).unwrap();
    file.flush().unwrap();

    skill_path
}

#[test]
fn test_read_skill_executor_success() {
    let temp_dir = TempDir::new().unwrap();
    let skill_path = create_test_skill_file(&temp_dir, "test-skill", "A test skill");

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Populate SkillManager cache with the test skill
        let parsed_skill = parse_skill(&skill_path).expect("Failed to parse test skill");
        SkillManager::handle(&app).update(&mut app, |manager, _ctx| {
            manager.add_skill_for_testing(parsed_skill);
        });

        let executor_handle = add_test_read_skill_executor(&mut app);

        let action = AIAgentAction {
            id: AIAgentActionId::from("test-action-id".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(skill_path.clone().into()),
            }),
            task_id: TaskId::new("test-task-id".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();

            match result {
                AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                    ReadSkillResult::Success { content },
                )) => {
                    assert_eq!(content.file_name, skill_path.to_string_lossy().to_string());
                }
                _ => panic!("Successfully read skill file; should return ReadSkillResult::Success"),
            }
        });
    });
}

#[test]
fn disconnected_remote_session_does_not_fall_back_to_client_global_bundled_skill() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let _bundled_skills = FeatureFlag::BundledSkills.override_enabled(true);
        SkillManager::handle(&app).update(&mut app, |manager, _ctx| {
            manager.add_bundled_skill_for_testing(
                "remote-only",
                bundled_skill("remote-only"),
                BundledSkillActivation::Always,
            );
        });
        let executor_handle = add_remote_read_skill_executor(&mut app, None);

        let action = read_skill_action(SkillReference::BundledSkillId("remote-only".to_string()));
        assert_eq!(
            execute_sync_read(&mut app, &executor_handle, &action),
            Err("Bundled skills are not available on this remote session".to_string())
        );
    });
}

#[test]
fn remote_session_reads_remote_bundled_skill_catalog() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let _bundled_skills = FeatureFlag::BundledSkills.override_enabled(true);
        let host_id = HostId::new("remote-host".to_string());
        let remote_skill = ParsedSkill {
            name: "host-specific".to_string(),
            description: "remote bundled skill".to_string(),
            path: remote_skill_path(
                &host_id,
                "/opt/warp/resources/bundled/skills/host-specific/SKILL.md",
            ),
            content: "remote rendered content".to_string(),
            line_range: None,
            provider: SkillProvider::Zap,
            scope: SkillScope::Bundled,
        };
        SkillManager::handle(&app).update(&mut app, |manager, _ctx| {
            manager.add_bundled_skill_for_testing(
                "host-specific",
                bundled_skill("host-specific"),
                BundledSkillActivation::Always,
            );
            manager.add_remote_bundled_skill_for_testing(
                host_id.clone(),
                "host-specific",
                remote_skill,
                BundledSkillActivation::Always,
            );
        });
        let executor_handle = add_remote_read_skill_executor(&mut app, Some(host_id));

        let action = read_skill_action(SkillReference::BundledSkillId("host-specific".to_string()));
        assert_eq!(
            execute_sync_read(&mut app, &executor_handle, &action),
            Ok((
                "/opt/warp/resources/bundled/skills/host-specific/SKILL.md".to_string(),
                AnyFileContent::StringContent("remote rendered content".to_string()),
            )),
            "Remote session should read its host-specific bundled skill"
        );
    });
}

#[test]
fn test_read_skill_executor_reads_enabled_bundled_skill() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let _bundled_skills = FeatureFlag::BundledSkills.override_enabled(true);
        SkillManager::handle(&app).update(&mut app, |manager, _ctx| {
            manager.add_bundled_skill_for_testing(
                "pr-comments",
                bundled_skill("pr-comments"),
                BundledSkillActivation::Always,
            );
        });
        let executor_handle = add_test_read_skill_executor(&mut app);

        let action = read_skill_action(SkillReference::BundledSkillId("pr-comments".to_string()));
        let (file_name, _) = execute_sync_read(&mut app, &executor_handle, &action)
            .expect("Enabled bundled skill should return ReadSkillResult::Success");
        assert_eq!(file_name, "/bundled/skills/pr-comments/SKILL.md");
    });
}

#[test]
fn test_read_skill_executor_rejects_warp_control_bundled_skills_when_disabled() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let _bundled_skills = FeatureFlag::BundledSkills.override_enabled(true);
        let _warp_control_cli = FeatureFlag::WarpControlCli.override_enabled(false);
        let skill_id = "warpctrl";
        SkillManager::handle(&app).update(&mut app, |manager, _ctx| {
            manager.add_bundled_skill_for_testing(
                skill_id,
                bundled_skill(skill_id),
                BundledSkillActivation::RequiresFeature(FeatureFlag::WarpControlCli),
            );
        });
        let executor_handle = add_test_read_skill_executor(&mut app);

        let action = read_skill_action(SkillReference::BundledSkillId(skill_id.to_string()));
        assert!(execute_sync_read(&mut app, &executor_handle, &action).is_err());
    });
}

#[test]
fn test_read_skill_executor_file_not_found() {
    let temp_dir = TempDir::new().unwrap();
    // Don't create the SKILL.md file
    let skill_path = temp_dir.path().join("SKILL.md");

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let executor_handle = add_test_read_skill_executor(&mut app);

        let action = AIAgentAction {
            id: AIAgentActionId::from("test-action-id".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(skill_path.into()),
            }),
            task_id: TaskId::new("test-task-id".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();

            match result {
                AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                    ReadSkillResult::Error(error_msg),
                )) => {
                    // Should contain an error about file not found or I/O error
                    assert!(!error_msg.is_empty());
                }
                _ => panic!(
                    "Nonexistent SKILL.md file at given path; should return ReadSkillResult::Error"
                ),
            }
        });
    });
}

/// Issue #99 fallback: on a cache miss, if SkillReference::Path points to a validly shaped skill file,
/// read from disk directly and return successfully (taking the Async branch).
#[test]
fn test_read_skill_executor_fallback_reads_disk_on_cache_miss() {
    let temp_dir = TempDir::new().unwrap();
    let skill_path = create_test_skill_file(&temp_dir, "fallback-skill", "Read from disk");

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        // Note: don't call add_skill_for_testing, to simulate a cache miss.
        let executor_handle = add_test_read_skill_executor(&mut app);

        let action = AIAgentAction {
            id: AIAgentActionId::from("fallback-action".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(skill_path.clone().into()),
            }),
            task_id: TaskId::new("fallback-task".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        let execution = executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Async {
            execute_future,
            on_complete,
        } = execution
        else {
            panic!("Cache miss with valid skill path should produce Async execution");
        };

        let async_result = execute_future.await;
        let result = app.update(|ctx| on_complete(async_result, ctx));

        match result {
            AIAgentActionResultType::ReadSkill(ReadSkillResult::Success { content }) => {
                assert_eq!(content.file_name, skill_path.to_string_lossy().to_string());
                let body = match &content.content {
                    AnyFileContent::StringContent(s) => s.clone(),
                    AnyFileContent::BinaryContent(_) => {
                        panic!("SKILL.md should be parsed as text")
                    }
                };
                assert!(body.contains("fallback-skill"));
            }
            other => panic!("Fallback should return Success, got: {other:?}"),
        }
    });
}

/// Issue #99 fallback failure path: on a cache miss, if the path shape is valid but the file doesn't exist on disk
/// (e.g. a race where it was deleted after validation), the Async branch's parse_skill fails and on_complete should return Error.
#[test]
fn test_read_skill_executor_fallback_returns_error_when_file_missing() {
    let temp_dir = TempDir::new().unwrap();
    // The path shape is valid, but SKILL.md was never created.
    let skill_path = temp_dir
        .path()
        .join(".agents/skills/missing-skill/SKILL.md");

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let executor_handle = add_test_read_skill_executor(&mut app);

        let action = AIAgentAction {
            id: AIAgentActionId::from("missing-action".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(skill_path.into()),
            }),
            task_id: TaskId::new("missing-task".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        let execution = executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Async {
            execute_future,
            on_complete,
        } = execution
        else {
            panic!(
                "Legal-shaped skill path should still produce Async execution before disk check"
            );
        };

        let async_result = execute_future.await;
        let result = app.update(|ctx| on_complete(async_result, ctx));

        match result {
            AIAgentActionResultType::ReadSkill(ReadSkillResult::Error(msg)) => {
                assert!(msg.starts_with("Skill not found"));
            }
            other => panic!("Missing file should resolve to Error, got: {other:?}"),
        }
    });
}

/// When the BYOP `read_skill` tool is called by name:
/// `from_args` packs the name into `SkillReference::SkillPath(name)`,
/// and after a cache miss on the executor side, it looks up by name, hits, and returns Sync Success.
#[test]
fn test_read_skill_executor_resolves_by_name() {
    let temp_dir = TempDir::new().unwrap();
    let skill_path = create_test_skill_file(&temp_dir, "byop-named-skill", "Lookup by name");

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let parsed_skill = parse_skill(&skill_path).expect("Failed to parse test skill");
        SkillManager::handle(&app).update(&mut app, |manager, _ctx| {
            manager.add_skill_for_testing(parsed_skill);
        });

        let executor_handle = add_test_read_skill_executor(&mut app);

        // Simulate BYOP from_args: pass the name in as the path.
        let action = AIAgentAction {
            id: AIAgentActionId::from("name-lookup-action".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(std::path::PathBuf::from("byop-named-skill").into()),
            }),
            task_id: TaskId::new("name-lookup-task".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            match result {
                AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                    ReadSkillResult::Success { content },
                )) => {
                    assert_eq!(content.file_name, skill_path.to_string_lossy().to_string());
                }
                _ => panic!("Lookup by name should succeed via Sync Success"),
            }
        });
    });
}

/// The BYOP name lookup respects bundled-skill activation like `BundledSkillId` reads do:
/// a disabled bundled skill is not readable by name, and becomes readable once enabled.
#[test]
fn test_read_skill_executor_by_name_respects_bundled_skill_activation() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let _bundled_skills = FeatureFlag::BundledSkills.override_enabled(true);
        let warp_control_cli = FeatureFlag::WarpControlCli.override_enabled(false);
        SkillManager::handle(&app).update(&mut app, |manager, _ctx| {
            manager.add_bundled_skill_for_testing(
                "warpctrl",
                bundled_skill("warpctrl"),
                BundledSkillActivation::RequiresFeature(FeatureFlag::WarpControlCli),
            );
        });
        let executor_handle = add_test_read_skill_executor(&mut app);
        let action = read_skill_action(byop_name_reference("warpctrl"));

        let disabled = execute_sync_read(&mut app, &executor_handle, &action);
        assert!(
            matches!(&disabled, Err(message) if message.starts_with("Skill not found")),
            "a disabled bundled skill must not be readable by name, got: {disabled:?}"
        );

        drop(warp_control_cli);
        let _warp_control_cli_enabled = FeatureFlag::WarpControlCli.override_enabled(true);
        let (file_name, _) = execute_sync_read(&mut app, &executor_handle, &action)
            .expect("an enabled bundled skill should be readable by name");
        assert_eq!(file_name, "/bundled/skills/warpctrl/SKILL.md");
    });
}

/// In a remote session the BYOP name lookup reads the remote host's bundled catalog, never the
/// client's same-named bundled skill.
#[test]
fn remote_session_reads_remote_bundled_skill_by_name() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let _bundled_skills = FeatureFlag::BundledSkills.override_enabled(true);
        let host_id = HostId::new("remote-host".to_string());
        let remote_skill = ParsedSkill {
            path: remote_skill_path(
                &host_id,
                "/opt/warp/resources/bundled/skills/host-specific/SKILL.md",
            ),
            content: "remote rendered content".to_string(),
            ..bundled_skill("host-specific")
        };
        SkillManager::handle(&app).update(&mut app, |manager, _ctx| {
            manager.add_bundled_skill_for_testing(
                "host-specific",
                bundled_skill("host-specific"),
                BundledSkillActivation::Always,
            );
            manager.add_remote_bundled_skill_for_testing(
                host_id.clone(),
                "host-specific",
                remote_skill,
                BundledSkillActivation::Always,
            );
        });
        let executor_handle = add_remote_read_skill_executor(&mut app, Some(host_id));

        let action = read_skill_action(byop_name_reference("host-specific"));
        assert_eq!(
            execute_sync_read(&mut app, &executor_handle, &action),
            Ok((
                "/opt/warp/resources/bundled/skills/host-specific/SKILL.md".to_string(),
                AnyFileContent::StringContent("remote rendered content".to_string()),
            ))
        );
    });
}

/// The BYOP name lookup only considers file skills on the active session's host: a local
/// session reads the local skill and a remote session reads the remote host's skill, even when
/// the other host's same-named skill comes from a higher-priority provider.
#[test]
fn test_read_skill_executor_resolves_name_on_the_session_host() {
    let temp_dir = TempDir::new().unwrap();
    let local_skill_path = create_test_skill_file(&temp_dir, "deploy", "Local deploy");

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let host_id = HostId::new("remote-host".to_string());
        let local_skill = parse_skill(&local_skill_path).expect("Failed to parse test skill");
        // `.agents` outranks the local skill's `.claude` provider.
        let remote_skill = ParsedSkill {
            name: "deploy".to_string(),
            description: "Remote deploy".to_string(),
            path: remote_skill_path(&host_id, "/repo/.agents/skills/deploy/SKILL.md"),
            content: "# deploy".to_string(),
            line_range: None,
            provider: SkillProvider::Agents,
            scope: SkillScope::Project,
        };
        SkillManager::handle(&app).update(&mut app, |manager, _ctx| {
            manager.add_skill_for_testing(local_skill);
            manager.add_skill_for_testing(remote_skill);
        });
        let local_executor = add_test_read_skill_executor(&mut app);
        let remote_executor = add_remote_read_skill_executor(&mut app, Some(host_id));
        let action = read_skill_action(byop_name_reference("deploy"));

        let (local_file_name, _) = execute_sync_read(&mut app, &local_executor, &action)
            .expect("a local session should read the local skill by name");
        assert_eq!(
            local_file_name,
            local_skill_path.to_string_lossy().to_string()
        );

        let (remote_file_name, _) = execute_sync_read(&mut app, &remote_executor, &action)
            .expect("a remote session should read the remote host's skill by name");
        assert_eq!(remote_file_name, "/repo/.agents/skills/deploy/SKILL.md");
    });
}

/// For an unknown name (not in the SkillManager index), after exhausting all fallbacks:
/// `name_candidate` hits but `find_skill_by_name` returns None, continuing to the fs fallback —
/// here the path shape is invalid (a bare name with no `/`), so it returns Sync Error directly.
#[test]
fn test_read_skill_executor_rejects_unknown_name() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let executor_handle = add_test_read_skill_executor(&mut app);

        let action = AIAgentAction {
            id: AIAgentActionId::from("unknown-name-action".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(std::path::PathBuf::from("no-such-skill").into()),
            }),
            task_id: TaskId::new("unknown-name-task".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            match result {
                AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                    ReadSkillResult::Error(msg),
                )) => {
                    assert!(msg.starts_with("Skill not found"), "msg={msg}");
                }
                _ => panic!("Unknown name should resolve to Sync Error"),
            }
        });
    });
}

/// Issue #99 safety gate: on a cache miss, if the path doesn't match the skill file shape,
/// take the Sync Error branch directly, triggering no disk read.
#[test]
fn test_read_skill_executor_rejects_non_skill_path_on_cache_miss() {
    let temp_dir = TempDir::new().unwrap();
    // A random markdown file that is not in the `.<provider>/skills/<name>/SKILL.md` structure.
    // Even if this file exists, the fallback should not read it —— extract_skill_parent_directory will reject it.
    let non_skill_path = temp_dir.path().join("random.md");
    fs::write(&non_skill_path, "not a skill").unwrap();

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let executor_handle = add_test_read_skill_executor(&mut app);

        let action = AIAgentAction {
            id: AIAgentActionId::from("non-skill-action".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(non_skill_path.into()),
            }),
            task_id: TaskId::new("non-skill-task".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            match result {
                AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                    ReadSkillResult::Error(msg),
                )) => {
                    assert!(msg.starts_with("Skill not found"));
                }
                _ => panic!(
                    "Non-skill path on cache miss should return Sync Error, not Async fallback"
                ),
            }
        });
    });
}
