//! Recovery decisions for a failed response-stream attempt.
//!
//! Upstream unit-tests a pure `recovery_action` over a shared `RecoveryBudget`. The fork's BYOP
//! stream keeps the recovery policy inline in `handle_response_stream_event`, so these tests
//! drive a test stream through the same scenarios and assert what it does: retry in-request,
//! schedule a conversation resume, or surface the error.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use warp_multi_agent_api::response_event;
use warpui::{App, ModelHandle, SingletonEntity};

use super::{MAX_RETRIES, ResponseStream, ResponseStreamEvent, ResponseStreamId};
use crate::ai::api_error::AIApiError;
use crate::network::NetworkStatus;

/// Errors the stream surfaced to its subscriber instead of recovering in-request.
type SurfacedErrors = Rc<RefCell<Vec<Arc<AIApiError>>>>;

/// A transient transport failure on a BYOP stream: retryable.
fn transient_error() -> Arc<AIApiError> {
    Arc::new(AIApiError::Stream {
        stream_type: "byop",
        source: anyhow::anyhow!("stream ended before the response finished"),
    })
}

/// A deterministic client error (e.g. an invalid request): not retryable.
fn client_error() -> Arc<AIApiError> {
    Arc::new(AIApiError::ErrorStatus(
        http::StatusCode::BAD_REQUEST,
        "invalid request".to_owned(),
    ))
}

fn client_actions_event() -> warp_multi_agent_api::ResponseEvent {
    warp_multi_agent_api::ResponseEvent {
        r#type: Some(response_event::Type::ClientActions(
            response_event::ClientActions { actions: vec![] },
        )),
    }
}

fn set_online(app: &mut App, online: bool) {
    NetworkStatus::handle(&*app).update(app, |network_status, ctx| {
        network_status.reachability_changed(online, ctx);
    });
}

/// Adds a test stream (which never spawns a real request) and captures the errors it surfaces.
///
/// `can_attempt_resume_on_error` is false for requests that are themselves an automatic
/// resume, which bounds recovery to a single resume.
fn add_test_stream(
    app: &mut App,
    can_attempt_resume_on_error: bool,
) -> (ModelHandle<ResponseStream>, SurfacedErrors) {
    let stream = app.add_model(|_| {
        let mut stream = ResponseStream::new_for_test(ResponseStreamId::new_for_test());
        stream.can_attempt_resume_on_error = can_attempt_resume_on_error;
        stream
    });
    let surfaced = SurfacedErrors::default();
    let surfaced_for_subscription = Rc::clone(&surfaced);
    app.update(|ctx| {
        ctx.subscribe_to_model(&stream, move |_, event, _| {
            if let ResponseStreamEvent::ReceivedEvent(event) = event
                && let Some(Err(error)) = event.consume()
            {
                surfaced_for_subscription.borrow_mut().push(error);
            }
        });
    });
    (stream, surfaced)
}

#[test]
fn pre_action_failures_retry() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| NetworkStatus::new());

        // Resume eligibility is irrelevant pre-actions.
        for can_attempt_resume_on_error in [true, false] {
            let (stream, surfaced) = add_test_stream(&mut app, can_attempt_resume_on_error);
            stream.update(&mut app, |stream, ctx| {
                stream.emit_error_event_for_test(transient_error(), ctx);
            });

            assert!(
                surfaced.borrow().is_empty(),
                "a failure retried in-request must not be surfaced"
            );
            stream.read(&app, |stream, _| {
                assert_eq!(stream.retry_count, 1);
                assert!(!stream.should_resume_conversation_after_stream_finished());
            });
        }
    });
}

#[test]
fn pre_action_budget_exhaustion_is_terminal() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| NetworkStatus::new());

        // The request has already been retried MAX_RETRIES times; stop, online or not.
        for online in [true, false] {
            set_online(&mut app, online);
            let (stream, surfaced) = add_test_stream(&mut app, true);
            stream.update(&mut app, |stream, ctx| {
                stream.exhaust_recovery_budget_for_test(ctx);
                stream.emit_error_event_for_test(transient_error(), ctx);
            });

            assert_eq!(surfaced.borrow().len(), 1);
            stream.read(&app, |stream, _| {
                assert_eq!(stream.retry_count, MAX_RETRIES);
                assert!(!stream.should_resume_conversation_after_stream_finished());
            });
        }
    });
}

#[test]
fn non_recoverable_pre_action_failure_is_terminal() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| NetworkStatus::new());

        let (stream, surfaced) = add_test_stream(&mut app, true);
        stream.update(&mut app, |stream, ctx| {
            stream.emit_error_event_for_test(client_error(), ctx);
        });

        assert_eq!(surfaced.borrow().len(), 1);
        stream.read(&app, |stream, _| {
            assert_eq!(stream.retry_count, 0);
            assert!(!stream.should_resume_conversation_after_stream_finished());
        });
    });
}

#[test]
fn post_action_recoverable_failures_resume() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| NetworkStatus::new());

        // Offline doesn't change the decision; the resume waits for connectivity. The
        // in-request retry budget is irrelevant once actions have executed.
        for (online, exhaust_retry_budget) in [(true, false), (false, false), (true, true)] {
            set_online(&mut app, online);
            let (stream, surfaced) = add_test_stream(&mut app, true);
            stream.update(&mut app, |stream, ctx| {
                if exhaust_retry_budget {
                    stream.exhaust_recovery_budget_for_test(ctx);
                }
                stream.emit_response_event_for_test(client_actions_event(), ctx);
                stream.emit_error_event_for_test(transient_error(), ctx);
            });

            // Re-sending is unsafe once actions have streamed, so the failure is surfaced and
            // the conversation is resumed with a fresh request after the stream finishes.
            assert_eq!(surfaced.borrow().len(), 1);
            stream.read(&app, |stream, _| {
                assert!(stream.should_resume_conversation_after_stream_finished());
            });
        }
    });
}

#[test]
fn post_action_failures_without_resume_eligibility_are_terminal() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| NetworkStatus::new());

        // Resume requests themselves run with can_attempt_resume_on_error=false, bounding
        // recovery to a single resume.
        let (stream, surfaced) = add_test_stream(&mut app, false);
        stream.update(&mut app, |stream, ctx| {
            stream.emit_response_event_for_test(client_actions_event(), ctx);
            stream.emit_error_event_for_test(transient_error(), ctx);
        });

        assert_eq!(surfaced.borrow().len(), 1);
        stream.read(&app, |stream, _| {
            assert_eq!(stream.retry_count, 0);
            assert!(!stream.should_resume_conversation_after_stream_finished());
        });
    });
}

#[test]
fn non_recoverable_post_action_failure_is_terminal() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| NetworkStatus::new());

        // A non-recoverable error (e.g. a client error) ends the conversation even
        // after actions have executed.
        let (stream, surfaced) = add_test_stream(&mut app, true);
        stream.update(&mut app, |stream, ctx| {
            stream.emit_response_event_for_test(client_actions_event(), ctx);
            stream.emit_error_event_for_test(client_error(), ctx);
        });

        assert_eq!(surfaced.borrow().len(), 1);
        stream.read(&app, |stream, _| {
            assert!(!stream.should_resume_conversation_after_stream_finished());
        });
    });
}
