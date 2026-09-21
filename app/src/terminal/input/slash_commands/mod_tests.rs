use super::{
    SlashCommandSelectionBehavior, parse_tab_color_argument, slash_command_is_submitted_as_prompt,
    slash_command_is_supported_in_tui, slash_command_selection_behavior,
};
use crate::search::slash_command_menu::static_commands::commands;
use crate::tab::SelectedTabColor;
use crate::ui_components::color_dot;

#[test]
fn local_gui_commands_request_arguments_and_never_become_prompts() {
    for command in [&*commands::RENAME_CONVERSATION, &*commands::SET_TAB_COLOR] {
        assert!(!slash_command_is_submitted_as_prompt(command));
        assert!(!slash_command_is_supported_in_tui(command));
        assert_eq!(
            slash_command_selection_behavior(command),
            SlashCommandSelectionBehavior::InsertCommandText(format!("{} ", command.name))
        );
    }
}

#[test]
fn tab_color_argument_accepts_palette_case_and_explicit_clear() {
    for color in color_dot::TAB_COLOR_OPTIONS {
        for argument in [color.to_string(), color.to_string().to_ascii_lowercase()] {
            assert_eq!(
                parse_tab_color_argument(Some(&format!(" {argument} "))),
                Ok(SelectedTabColor::Color(color))
            );
        }
    }
    for argument in ["none", " NONE ", "None"] {
        assert_eq!(
            parse_tab_color_argument(Some(argument)),
            Ok(SelectedTabColor::Cleared)
        );
    }
}

#[test]
fn tab_color_argument_rejects_missing_unknown_and_non_palette_colors() {
    for argument in [
        None,
        Some(""),
        Some("   "),
        Some("unknown"),
        Some("black"),
        Some("red blue"),
    ] {
        let message = parse_tab_color_argument(argument).unwrap_err();
        for color in color_dot::TAB_COLOR_OPTIONS {
            assert!(message.contains(&color.to_string().to_ascii_lowercase()));
        }
        assert!(message.contains("none"));
    }
}

#[cfg(all(feature = "local_fs", not(target_family = "wasm")))]
mod gui {
    use chrono::Local;
    use warp_core::features::FeatureFlag;
    use warpui::{App, ReadModel, ReadView, SingletonEntity, UpdateView};

    use super::*;
    use crate::ai::agent::{
        AIAgentExchange, AIAgentExchangeId, AIAgentInput, AIAgentOutputStatus, UserQueryMode,
    };
    use crate::ai::blocklist::agent_view::AgentViewEntryOrigin;
    use crate::ai::blocklist::{BlocklistAIHistoryModel, ResponseStreamId};
    use crate::ai::llms::LLMPreferences;
    use crate::terminal::input::slash_commands::SlashCommandTrigger;
    use crate::terminal::input::tests::{add_window_with_bootstrapped_terminal, initialize_app};

    #[test]
    fn rename_conversation_slash_preserves_rejected_drafts_and_updates_local_history() {
        App::test((), |mut app| async move {
            let _flag = FeatureFlag::AgentView.override_enabled(true);
            initialize_app(&mut app);
            let terminal = add_window_with_bootstrapped_terminal(&mut app, None, None).await;
            let input = terminal.read(&app, |view, _| view.input().clone());
            let terminal_id = terminal.read(&app, |view, _| view.id());
            let conversation_id = terminal.update(&mut app, |view, ctx| {
                view.agent_view_controller().update(ctx, |controller, ctx| {
                    controller
                        .try_enter_agent_view(
                            None,
                            AgentViewEntryOrigin::Input {
                                was_prompt_autodetected: false,
                            },
                            ctx,
                        )
                        .unwrap()
                })
            });

            input.update(&mut app, |input, ctx| {
                let draft = "/rename-conversation Empty conversation";
                input.replace_buffer_content(draft, ctx);
                assert!(input.execute_slash_command(
                    &commands::RENAME_CONVERSATION,
                    Some(&"Empty conversation".to_owned()),
                    SlashCommandTrigger::input(),
                    false,
                    None,
                    None,
                    ctx,
                ));
                assert_eq!(input.buffer_text(ctx), draft);
            });

            // No inference request is needed to exercise the GUI command.
            let history = BlocklistAIHistoryModel::handle(&app);
            history.update(&mut app, |history, ctx| {
                let model_id = LLMPreferences::as_ref(ctx)
                    .get_active_base_model(ctx, Some(terminal_id))
                    .id
                    .clone();
                history
                    .conversation_mut(&conversation_id)
                    .unwrap()
                    .append_reassigned_exchange(
                        &ResponseStreamId::new_for_test(),
                        AIAgentExchange {
                            id: AIAgentExchangeId::new(),
                            input: vec![AIAgentInput::UserQuery {
                                query: "Synthetic local request".to_owned(),
                                context: Default::default(),
                                static_query_type: None,
                                referenced_attachments: Default::default(),
                                user_query_mode: UserQueryMode::Normal,
                                running_command: None,
                                intended_agent: None,
                            }],
                            output_status: AIAgentOutputStatus::Streaming { output: None },
                            added_message_ids: Default::default(),
                            start_time: Local::now(),
                            finish_time: None,
                            time_to_first_token_ms: None,
                            working_directory: None,
                            model_id: model_id.clone(),
                            coding_model_id: model_id.clone(),
                            cli_agent_model_id: model_id.clone(),
                            computer_use_model_id: model_id,
                            request_cost: None,
                            response_initiator: None,
                        },
                        terminal_id,
                        ctx,
                    )
                    .unwrap();
            });
            let original_title = history.read(&app, |history, _| {
                history.conversation(&conversation_id).unwrap().title()
            });
            for argument in ["   ".to_owned(), "x".repeat(501)] {
                input.update(&mut app, |input, ctx| {
                    let draft = format!("/rename-conversation {argument}");
                    input.replace_buffer_content(&draft, ctx);
                    assert!(input.execute_slash_command(
                        &commands::RENAME_CONVERSATION,
                        Some(&argument),
                        SlashCommandTrigger::input(),
                        false,
                        None,
                        None,
                        ctx,
                    ));
                    assert_eq!(input.buffer_text(ctx), draft);
                });
            }
            history.read(&app, |history, _| {
                assert_eq!(
                    history.conversation(&conversation_id).unwrap().title(),
                    original_title
                );
            });
            input.update(&mut app, |input, ctx| {
                input.replace_buffer_content("/rename-conversation   Local café  ", ctx);
                assert!(input.execute_slash_command(
                    &commands::RENAME_CONVERSATION,
                    Some(&"  Local café  ".to_owned()),
                    SlashCommandTrigger::input(),
                    false,
                    None,
                    None,
                    ctx
                ));
                assert!(input.buffer_text(ctx).is_empty());
            });
            history.read(&app, |history, _| {
                let conversation = history.conversation(&conversation_id).unwrap();
                assert_eq!(conversation.title().as_deref(), Some("Local café"));
                assert_eq!(
                    conversation.get_root_task().unwrap().description(),
                    "Local café"
                );
            });
        });
    }
}

/// The centralized classifier must mark only the prompt-submitting commands (/compact, /plan,
/// /orchestrate) as "submitted as a prompt". Every other slash command emits an immediate action
/// and must be treated as "run now" by the prompt-queue gate and the shared-session viewer path.
#[test]
fn slash_command_is_submitted_as_prompt_only_for_prompt_commands() {
    // Prompt-submitting commands reiterate their text into the conversation.
    assert!(slash_command_is_submitted_as_prompt(&commands::COMPACT));
    assert!(slash_command_is_submitted_as_prompt(&commands::PLAN));
    assert!(slash_command_is_submitted_as_prompt(&commands::ORCHESTRATE));

    // Action-emitting commands run immediately and are never queued / forwarded as prompts.
    assert!(!slash_command_is_submitted_as_prompt(&commands::FORK));
    assert!(!slash_command_is_submitted_as_prompt(
        &commands::FORK_AND_COMPACT
    ));
    assert!(!slash_command_is_submitted_as_prompt(&commands::FORK_FROM));
    assert!(!slash_command_is_submitted_as_prompt(
        &commands::COMPACT_AND
    ));
    assert!(!slash_command_is_submitted_as_prompt(&commands::MODEL));
    assert!(!slash_command_is_submitted_as_prompt(&commands::REWIND));
    assert!(!slash_command_is_submitted_as_prompt(
        &commands::CONVERSATIONS
    ));
    assert!(!slash_command_is_submitted_as_prompt(&commands::QUEUE));
}

#[cfg(all(feature = "local_fs", windows))]
mod windows {
    use std::sync::Arc;

    use super::super::*;
    use crate::terminal::ShellLaunchData;
    use crate::terminal::model::session::SessionInfo;
    use crate::terminal::model::session::command_executor::testing::TestCommandExecutor;
    use crate::terminal::shell::ShellType;

    fn wsl_session() -> Session {
        Session::new(
            SessionInfo::new_for_test().with_shell_type(ShellType::Bash),
            Arc::new(TestCommandExecutor::default()),
        )
        .with_shell_launch_data(ShellLaunchData::WSL {
            distro: "Ubuntu".to_owned(),
        })
    }

    #[test]
    fn open_file_command_converts_wsl_paths_to_host_paths() {
        let session = wsl_session();
        let cases = [
            (
                "/home/ubuntu",
                "subdir/test.txt",
                r"\\WSL$\Ubuntu\home\ubuntu\subdir\test.txt",
                None,
            ),
            (
                "/home/ubuntu/project",
                "../test.txt",
                r"\\WSL$\Ubuntu\home\ubuntu\test.txt",
                None,
            ),
            (
                "/home/ubuntu",
                "subdir/file\\ name.txt",
                r"\\WSL$\Ubuntu\home\ubuntu\subdir\file name.txt",
                None,
            ),
            (
                "/home/ubuntu",
                "subdir/test.txt:4:2",
                r"\\WSL$\Ubuntu\home\ubuntu\subdir\test.txt",
                Some(LineAndColumnArg {
                    line_num: 4,
                    column_num: Some(2),
                }),
            ),
        ];

        for (current_dir, raw_arg, expected_path, expected_line_col) in cases {
            let (path, line_col) = open_file_command_path(&session, current_dir, raw_arg);

            assert_eq!(path, PathBuf::from(expected_path));
            assert_eq!(line_col, expected_line_col);
        }
    }
}
