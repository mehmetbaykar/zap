use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Local;
use warp_multi_agent_api::response_event;
use warpui::{App, ModelHandle, SingletonEntity, ViewHandle};

use crate::ai::agent::conversation::{AIConversationId, ConversationStatus};
use crate::ai::agent::task::TaskId;
use crate::ai::agent::{
    AIAgentAttachment, AIAgentContext, AIAgentInput, CancellationReason, ImageContext,
    RenderableAIError, UserQueryMode,
};
use crate::ai::api_error::AIApiError;
use crate::ai::blocklist::{
    BlocklistAIHistoryEvent, BlocklistAIHistoryModel, PendingAttachment, PendingFile, RequestInput,
    ResponseStream, ResponseStreamId,
};
use crate::ai::llms::LLMId;
use crate::network::NetworkStatus;
use crate::terminal::TerminalView;
use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};

fn image_attachment(file_name: &str) -> PendingAttachment {
    PendingAttachment::Image(ImageContext {
        data: String::new(),
        mime_type: "image/png".to_owned(),
        file_name: file_name.to_owned(),
        is_figma: false,
    })
}

fn file_attachment(file_name: &str) -> PendingAttachment {
    PendingAttachment::File(PendingFile {
        file_name: file_name.to_owned(),
        file_path: file_name.into(),
        mime_type: "text/plain".to_owned(),
    })
}

fn register_mock_response_stream(
    terminal: &ViewHandle<TerminalView>,
    app: &mut App,
) -> (AIConversationId, ModelHandle<ResponseStream>) {
    terminal.update(app, |view, ctx| {
        let terminal_surface_id = view.id();
        let stream_id = ResponseStreamId::new_for_test();
        let conversation_id = BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
            let conversation_id =
                history.start_new_conversation(terminal_surface_id, false, false, false, ctx);
            let task_id = history
                .conversation(&conversation_id)
                .unwrap()
                .get_root_task_id()
                .clone();
            history
                .update_conversation_for_new_request_input(
                    RequestInput {
                        conversation_id,
                        input_messages: HashMap::from([(task_id, vec![])]),
                        working_directory: None,
                        model_id: LLMId::from("test-model"),
                        coding_model_id: LLMId::from("test-coding-model"),
                        cli_agent_model_id: LLMId::from("test-cli-agent-model"),
                        computer_use_model_id: LLMId::from("test-computer-use-model"),
                        shared_session_response_initiator: None,
                        request_start_ts: Local::now(),
                        supported_tools_override: None,
                    },
                    stream_id.clone(),
                    terminal_surface_id,
                    ctx,
                )
                .unwrap();
            conversation_id
        });
        let stream = ctx.add_model(|_| ResponseStream::new_for_test(stream_id.clone()));
        view.ai_controller().update(ctx, |controller, ctx| {
            controller.register_mock_stream_for_test(
                stream_id,
                conversation_id,
                stream.clone(),
                ctx,
            );
        });
        (conversation_id, stream)
    })
}

fn stream_init_event(request_id: &str) -> warp_multi_agent_api::ResponseEvent {
    warp_multi_agent_api::ResponseEvent {
        r#type: Some(response_event::Type::Init(response_event::StreamInit {
            request_id: request_id.to_string(),
            conversation_id: "test-server-conversation".to_string(),
            run_id: String::new(),
        })),
    }
}

fn stream_client_actions_event() -> warp_multi_agent_api::ResponseEvent {
    warp_multi_agent_api::ResponseEvent {
        r#type: Some(response_event::Type::ClientActions(
            response_event::ClientActions { actions: vec![] },
        )),
    }
}

/// A transient transport failure on a BYOP stream (the provider stream was cut mid-response).
/// This is the fork's analogue of upstream's `AIApiError::UnexpectedEof`, which the fork's
/// `AIApiError` doesn't have; like it, the error is retryable.
fn stream_transport_error() -> Arc<AIApiError> {
    Arc::new(AIApiError::Stream {
        stream_type: "byop",
        source: anyhow::anyhow!("stream ended before the response finished"),
    })
}

/// Asserts that a terminal stream failure ended the conversation in `Error`.
///
/// Upstream classifies the failure into a cloud task `PlatformErrorCode` through
/// `local_agent_task_sync_model`, which the fork removed. The fork's local equivalent is the
/// structured error recorded on the conversation: a non-resuming, non-user error.
fn assert_terminal_stream_error(app: &App, conversation_id: AIConversationId) {
    BlocklistAIHistoryModel::handle(app).read(app, |history, _| {
        let conversation = history
            .conversation(&conversation_id)
            .expect("test conversation must exist");
        assert_eq!(conversation.status(), &ConversationStatus::Error);

        let error = conversation
            .status_error()
            .expect("terminal stream error must record a structured error");
        assert!(
            matches!(
                error,
                RenderableAIError::Other {
                    will_attempt_resume: false,
                    is_user_error: false,
                    ..
                }
            ),
            "unexpected terminal stream error: {error:?}"
        );
    });
}

#[test]
fn transport_failure_classification_is_independent_of_stream_start() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);

        let (started_conversation_id, started_stream) =
            register_mock_response_stream(&terminal, &mut app);
        started_stream.update(&mut app, |stream, ctx| {
            stream.exhaust_recovery_budget_for_test(ctx);
            stream.emit_response_event_for_test(stream_client_actions_event(), ctx);
            stream.emit_error_event_for_test(stream_transport_error(), ctx);
        });
        assert_terminal_stream_error(&app, started_conversation_id);

        let (retried_conversation_id, retried_stream) =
            register_mock_response_stream(&terminal, &mut app);
        retried_stream.update(&mut app, |stream, ctx| {
            stream.emit_response_event_for_test(stream_init_event("previous-attempt"), ctx);
            stream.exhaust_recovery_budget_for_test(ctx);
            stream.emit_error_event_for_test(stream_transport_error(), ctx);
        });
        assert_terminal_stream_error(&app, retried_conversation_id);
    });
}

#[test]
fn input_for_query_routes_queued_and_direct_attachments_independently() {
    // Queued sends use the row-owned attachment snapshot and ignore newer live staging, while
    // direct sends consume the context model's current pending attachments.
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);

        terminal.update(&mut app, |terminal, ctx| {
            let conversation_id =
                BlocklistAIHistoryModel::handle(ctx).update(ctx, |history_model, ctx| {
                    history_model.start_new_conversation(terminal.id(), false, false, false, ctx)
                });

            let controller = terminal.ai_controller();
            let context_model = controller.as_ref(ctx).context_model.clone();
            let active_session = controller.as_ref(ctx).active_session.clone();

            // Stage *live* attachments that must NOT leak into a query built from a different,
            // explicitly-provided attachment set.
            context_model.update(ctx, |m, ctx| {
                m.append_pending_attachments(
                    vec![image_attachment("live.png"), file_attachment("live.txt")],
                    ctx,
                );
            });

            let task_id = TaskId::new("test-task".to_owned());
            // Two files sharing a basename to exercise duplicate-basename suffixing.
            let prompt_attachments = vec![
                image_attachment("queued.png"),
                file_attachment("notes.txt"),
                file_attachment("notes.txt"),
            ];

            let input = super::input_for_query(
                "build a query".to_owned(),
                &task_id,
                conversation_id,
                None,
                UserQueryMode::Normal,
                None,
                HashMap::new(),
                prompt_attachments,
                true,
                context_model.as_ref(ctx),
                active_session.as_ref(ctx),
                ctx,
            );

            let AIAgentInput::UserQuery {
                context,
                referenced_attachments,
                ..
            } = input
            else {
                panic!("expected UserQuery");
            };

            // The provided image is attached as image context; the live-staged image is not.
            let image_names: Vec<&str> = context
                .iter()
                .filter_map(|c| match c {
                    AIAgentContext::Image(img) => Some(img.file_name.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(image_names, vec!["queued.png"]);

            // The provided files are attached as FilePathReference with duplicate-basename
            // suffixing; the live-staged file is not.
            let mut file_names: Vec<String> = referenced_attachments
                .values()
                .filter_map(|a| match a {
                    AIAgentAttachment::FilePathReference { file_name, .. } => {
                        Some(file_name.clone())
                    }
                    _ => None,
                })
                .collect();
            file_names.sort();
            assert_eq!(
                file_names,
                vec!["notes.txt".to_owned(), "notes.txt".to_owned()]
            );
            assert!(referenced_attachments.contains_key("notes.txt"));
            assert!(referenced_attachments.contains_key("notes.txt (1)"));
            assert!(!referenced_attachments.contains_key("live.txt"));

            // Direct sends resolve the live staging, as the send path does.
            let live_attachments = context_model.as_ref(ctx).pending_attachments().to_vec();
            let direct_input = super::input_for_query(
                "build a direct query".to_owned(),
                &task_id,
                conversation_id,
                None,
                UserQueryMode::Normal,
                None,
                HashMap::new(),
                live_attachments,
                false,
                context_model.as_ref(ctx),
                active_session.as_ref(ctx),
                ctx,
            );
            let AIAgentInput::UserQuery {
                context,
                referenced_attachments,
                ..
            } = direct_input
            else {
                panic!("expected UserQuery");
            };
            let image_names: Vec<&str> = context
                .iter()
                .filter_map(|context| match context {
                    AIAgentContext::Image(image) => Some(image.file_name.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(image_names, vec!["live.png"]);
            assert!(referenced_attachments.contains_key("live.txt"));
            assert!(!referenced_attachments.contains_key("notes.txt"));
        });
    });
}

#[test]
fn cancelling_conversation_aborts_pending_auto_resume() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);

        // An ID with no backing conversation: if the scheduled wait ever
        // completes, the resume is a harmless no-op.
        let conversation_id = AIConversationId::new();

        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                // The fork has no `schedule_auto_resume_after_error`: the response-stream
                // handler schedules an auto-resume inline, by waiting for connectivity and
                // then resuming the conversation. Register an equivalent pending resume.
                let wait_for_online = NetworkStatus::as_ref(ctx).wait_until_online();
                let handle = ctx.spawn(wait_for_online, move |me, _, ctx| {
                    me.pending_auto_resume_handles.remove(&conversation_id);
                    me.resume_conversation(
                        conversation_id,
                        /*can_attempt_resume_on_error*/ false,
                        /*is_auto_resume_after_error*/ true,
                        vec![],
                        ctx,
                    );
                });
                controller
                    .pending_auto_resume_handles
                    .insert(conversation_id, handle);
                assert!(
                    controller
                        .pending_auto_resume_handles
                        .contains_key(&conversation_id)
                );

                controller.cancel_conversation_progress(
                    conversation_id,
                    CancellationReason::ManuallyCancelled,
                    ctx,
                );
                assert!(
                    !controller
                        .pending_auto_resume_handles
                        .contains_key(&conversation_id)
                );
            });
        });
    });
}

#[test]
fn mock_response_stream_updates_history_through_controller() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        let captured_events = Arc::new(Mutex::new(Vec::new()));
        let events_for_subscription = Arc::clone(&captured_events);
        app.update(|ctx| {
            ctx.subscribe_to_model(&BlocklistAIHistoryModel::handle(ctx), move |_, event, _| {
                events_for_subscription.lock().unwrap().push(event.clone())
            });
        });

        let (conversation_id, stream) = register_mock_response_stream(&terminal, &mut app);

        stream.update(&mut app, |stream, ctx| {
            stream.emit_response_event_for_test(stream_init_event("test-request"), ctx);
            stream.emit_response_event_for_test(
                warp_multi_agent_api::ResponseEvent {
                    r#type: Some(response_event::Type::Finished(
                        response_event::StreamFinished {
                            reason: Some(response_event::stream_finished::Reason::Done(
                                response_event::stream_finished::Done {},
                            )),
                            conversation_usage_metadata: None,
                            token_usage: vec![],
                            should_refresh_model_config: false,
                            #[allow(deprecated)]
                            request_cost: None,
                            request_charges: None,
                        },
                    )),
                },
                ctx,
            );
        });

        BlocklistAIHistoryModel::handle(&app).read(&app, |history, _| {
            assert_eq!(
                history.conversation(&conversation_id).map(|c| c.status()),
                Some(&crate::ai::agent::conversation::ConversationStatus::Success)
            );
        });
        let events = captured_events.lock().unwrap();
        // The fork's analogue of upstream's `ConversationServerTokenAssigned`: emitted when the
        // conversation first receives its server conversation token during StreamInit.
        assert!(events.iter().any(|event| matches!(
            event,
            BlocklistAIHistoryEvent::ConversationAgentIdAssigned {
                conversation_id: id,
                ..
            } if *id == conversation_id
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            BlocklistAIHistoryEvent::UpdatedStreamingExchange {
                conversation_id: id,
                ..
            } if *id == conversation_id
        )));
    });
}

#[test]
fn explicit_stream_finished_failures_are_classified_without_init() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);

        let reasons = [
            response_event::stream_finished::Reason::Other(Default::default()),
            response_event::stream_finished::Reason::LlmUnavailable(Default::default()),
            response_event::stream_finished::Reason::ChatgptSubscriptionError(
                response_event::stream_finished::ChatGptSubscriptionError {
                    message: "subscription limit reached".to_owned(),
                    ..Default::default()
                },
            ),
            response_event::stream_finished::Reason::InternalError(
                response_event::stream_finished::InternalError {
                    message: "server stream failure".to_owned(),
                },
            ),
        ];
        for reason in reasons {
            let (conversation_id, stream) = register_mock_response_stream(&terminal, &mut app);
            stream.update(&mut app, |stream, ctx| {
                stream.emit_response_event_for_test(
                    warp_multi_agent_api::ResponseEvent {
                        r#type: Some(response_event::Type::Finished(
                            response_event::StreamFinished {
                                reason: Some(reason),
                                conversation_usage_metadata: None,
                                token_usage: vec![],
                                should_refresh_model_config: false,
                                #[allow(deprecated)]
                                request_cost: None,
                                request_charges: None,
                            },
                        )),
                    },
                    ctx,
                );
            });
            assert_terminal_stream_error(&app, conversation_id);
        }
    });
}

/// When an agent command exits the shell, the conversation must be finalized as
/// `Error` (not `Cancelled`), and a subsequent `ManuallyCancelled` (as fired by
/// the pane-close path) must not overwrite that failure.
#[test]
fn fail_conversation_due_to_shell_exit_reports_error_and_survives_manual_cancel() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);

        let conversation_id = terminal.update(&mut app, |view, ctx| {
            let terminal_surface_id = view.id();
            let stream_id = ResponseStreamId::new_for_test();
            let conversation_id =
                BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                    let conversation_id = history.start_new_conversation(
                        terminal_surface_id,
                        false,
                        false,
                        false,
                        ctx,
                    );
                    let task_id = history
                        .conversation(&conversation_id)
                        .unwrap()
                        .get_root_task_id()
                        .clone();
                    history
                        .update_conversation_for_new_request_input(
                            RequestInput {
                                conversation_id,
                                input_messages: HashMap::from([(task_id, vec![])]),
                                working_directory: None,
                                model_id: LLMId::from("test-model"),
                                coding_model_id: LLMId::from("test-coding-model"),
                                cli_agent_model_id: LLMId::from("test-cli-agent-model"),
                                computer_use_model_id: LLMId::from("test-computer-use-model"),
                                shared_session_response_initiator: None,
                                request_start_ts: Local::now(),
                                supported_tools_override: None,
                            },
                            stream_id.clone(),
                            terminal_surface_id,
                            ctx,
                        )
                        .unwrap();
                    conversation_id
                });
            let stream = ctx.add_model(|_| ResponseStream::new_for_test(stream_id.clone()));
            view.ai_controller().update(ctx, |controller, ctx| {
                controller.register_mock_stream_for_test(stream_id, conversation_id, stream, ctx);
                controller.fail_conversation_due_to_shell_exit(
                    conversation_id,
                    "exit 1".to_string(),
                    ctx,
                );
            });
            conversation_id
        });

        // The in-flight request is finalized as Error (with the shell-exit error
        // on its exchange), not Cancelled.
        BlocklistAIHistoryModel::handle(&app).read(&app, |history, _| {
            assert_eq!(
                history.conversation(&conversation_id).map(|c| c.status()),
                Some(&crate::ai::agent::conversation::ConversationStatus::Error)
            );
        });

        // The pane-close cancellation path must be a no-op now that the
        // conversation is terminal.
        terminal.update(&mut app, |view, ctx| {
            view.ai_controller().update(ctx, |controller, ctx| {
                controller.cancel_conversation_progress(
                    conversation_id,
                    CancellationReason::ManuallyCancelled,
                    ctx,
                );
            });
        });
        BlocklistAIHistoryModel::handle(&app).read(&app, |history, _| {
            assert_eq!(
                history.conversation(&conversation_id).map(|c| c.status()),
                Some(&crate::ai::agent::conversation::ConversationStatus::Error)
            );
        });
    });
}

/// An optimistic long-running-command completion that cancels an in-flight
/// stream must finalize the conversation as `Success`, not `Cancelled`. This is
/// a regression test for the reason -> status mapping living in a single place
/// (`CancellationReason::conversation_outcome`).
#[test]
fn optimistic_cli_subagent_completion_with_in_flight_stream_reports_success() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);

        let conversation_id = terminal.update(&mut app, |view, ctx| {
            let terminal_surface_id = view.id();
            let stream_id = ResponseStreamId::new_for_test();
            let conversation_id =
                BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                    let conversation_id = history.start_new_conversation(
                        terminal_surface_id,
                        false,
                        false,
                        false,
                        ctx,
                    );
                    let task_id = history
                        .conversation(&conversation_id)
                        .unwrap()
                        .get_root_task_id()
                        .clone();
                    history
                        .update_conversation_for_new_request_input(
                            RequestInput {
                                conversation_id,
                                input_messages: HashMap::from([(task_id, vec![])]),
                                working_directory: None,
                                model_id: LLMId::from("test-model"),
                                coding_model_id: LLMId::from("test-coding-model"),
                                cli_agent_model_id: LLMId::from("test-cli-agent-model"),
                                computer_use_model_id: LLMId::from("test-computer-use-model"),
                                shared_session_response_initiator: None,
                                request_start_ts: Local::now(),
                                supported_tools_override: None,
                            },
                            stream_id.clone(),
                            terminal_surface_id,
                            ctx,
                        )
                        .unwrap();
                    conversation_id
                });
            let stream = ctx.add_model(|_| ResponseStream::new_for_test(stream_id.clone()));
            view.ai_controller().update(ctx, |controller, ctx| {
                controller.register_mock_stream_for_test(stream_id, conversation_id, stream, ctx);
                // The long-running command finished while the agent was still
                // streaming, cancelling the in-flight stream optimistically.
                controller.cancel_conversation_progress(
                    conversation_id,
                    CancellationReason::CommandFinishedDuringInlineAgentView,
                    ctx,
                );
            });
            conversation_id
        });

        BlocklistAIHistoryModel::handle(&app).read(&app, |history, _| {
            assert_eq!(
                history.conversation(&conversation_id).map(|c| c.status()),
                Some(&crate::ai::agent::conversation::ConversationStatus::Success)
            );
        });
    });
}
