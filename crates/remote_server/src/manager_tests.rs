//! Pure-function-level unit tests for `manager.rs`.
//!
//! This only covers pure-function helpers —— it does not touch
//! `RemoteServerManager` itself, because the latter depends on
//! `warpui::Entity` / `ModelContext` and would require spinning up a whole
//! App context, which fits better in an integration testing framework.

use futures::channel::oneshot;
use warp_core::SessionId;
use warp_util::standardized_path::StandardizedPath;
use warpui_core::App;

use super::*;
use crate::HostId;
use crate::proto::{ClientMessage, RemoteAgentContextSnapshot, WriteFile, host_scoped_request};
use crate::protocol::RequestId;

// ---------------------------------------------------------------------------
// version_is_compatible
// ---------------------------------------------------------------------------

#[test]
fn version_compat_both_tagged_and_equal() {
    assert!(version_is_compatible(
        Some("v0.2026.05.10.stable"),
        "v0.2026.05.10.stable",
    ));
}

#[test]
fn version_compat_both_tagged_and_different() {
    assert!(!version_is_compatible(
        Some("v0.2026.05.10.stable"),
        "v0.2026.05.10.preview",
    ));
}

#[test]
fn version_compat_both_untagged() {
    // The client has no GIT_RELEASE_TAG (cargo run) and the server also
    // reports an empty string (`script/deploy_remote_server` dev deployment):
    // treat as compatible, keeping the local development loop unaffected.
    assert!(version_is_compatible(None, ""));
}

#[test]
fn version_compat_client_tagged_server_untagged() {
    // The client is a release and the server is a dev deployment → treat as
    // incompatible, correctly triggering the reinstall flow.
    assert!(!version_is_compatible(Some("v0.2026.05.10.stable"), ""));
}

#[test]
fn version_compat_client_untagged_server_tagged() {
    // **Key scenario**: the Zap client has no tag (cargo build), while the
    // server is a release pulled from the official CDN (with a tag). The
    // original helper judged this incompatible, which would trigger
    // `remove_remote_server_binary` → an infinite loop. This test only
    // records that `version_is_compatible`'s own behavior is unchanged; the
    // actual "skip the check" is handled by
    // [`should_enforce_remote_version_check`].
    assert!(!version_is_compatible(None, "v0.2026.05.10.stable"));
}

// ---------------------------------------------------------------------------
// should_enforce_remote_version_check
// ---------------------------------------------------------------------------

#[test]
fn enforce_version_check_skipped_on_oss() {
    // When Zap temporarily reuses the official release binary, the client and
    // server versions will never match, so the strict check must be skipped.
    assert!(!should_enforce_remote_version_check(Channel::Oss));
}

#[test]
fn enforce_version_check_kept_on_official_channels() {
    // On official channels the client and server either both come from the
    // same release CI run, or both come from a local `script/deploy_remote_server`
    // deployment, so the strict check is still necessary —— preserve the
    // original stale-binary self-healing path.
    for channel in [
        Channel::Stable,
        Channel::Preview,
        Channel::Dev,
        Channel::Local,
        Channel::Integration,
    ] {
        assert!(
            should_enforce_remote_version_check(channel),
            "channel {channel:?} should still enforce version check"
        );
    }
}

#[test]
fn abort_host_request_removes_pending_request_and_resolves_caller() {
    App::test((), |mut app| async move {
        let manager = app.add_model(RemoteServerManager::new);
        let host_id = HostId::new("test-host".to_string());
        let request_id = RequestId::new();
        let (result_tx, result_rx) = oneshot::channel();
        let msg = ClientMessage::host_scoped(
            request_id.to_string(),
            host_scoped_request::Message::WriteFile(WriteFile {
                path: "/tmp/test".to_string(),
                content: String::new(),
            }),
        );

        manager.update(&mut app, |manager, _ctx| {
            manager.pending_host_requests.insert(
                request_id.clone(),
                PendingHostRequest {
                    host_id,
                    dispatched_session_id: SessionId::from(1),
                    msg,
                    result_tx,
                    timeout_abort: None,
                },
            );
            manager.abort_host_request(&request_id);
            assert!(!manager.pending_host_requests.contains_key(&request_id));
        });

        assert!(matches!(
            result_rx.await.expect("manager should resolve caller"),
            Err(HostRequestError::Aborted)
        ));
    });
}

#[test]
fn remote_agent_context_snapshot_is_a_host_scoped_manager_event() {
    let host_id = HostId::new("test-host".to_string());
    let event = RemoteServerManagerEvent::RemoteAgentContextSnapshot {
        host_id,
        snapshot: RemoteAgentContextSnapshot {
            revision: 1,
            home_dir: "/home/user".to_string(),
            skills: Vec::new(),
            global_rules: Vec::new(),
        },
    };
    assert!(event.session_id().is_none());
}

#[test]
fn remote_agent_context_snapshot_revisions_are_deduplicated_per_host() {
    App::test((), |mut app| async move {
        let manager = app.add_model(RemoteServerManager::new);
        let host_id = HostId::new("test-host".to_string());
        let other_host_id = HostId::new("other-host".to_string());

        manager.update(&mut app, |manager, ctx| {
            assert!(manager.accept_remote_agent_context_snapshot_revision(&host_id, 2));
            assert!(!manager.accept_remote_agent_context_snapshot_revision(&host_id, 2));
            assert!(!manager.accept_remote_agent_context_snapshot_revision(&host_id, 1));
            assert!(manager.accept_remote_agent_context_snapshot_revision(&host_id, 3));
            assert!(manager.accept_remote_agent_context_snapshot_revision(&other_host_id, 1));

            manager.handle_host_disconnected(&host_id, ctx);
            assert!(manager.accept_remote_agent_context_snapshot_revision(&host_id, 3));
        });
    });
}

#[test]
fn start_ripgrep_search_without_connected_host_resolves_immediately() {
    App::test((), |mut app| async move {
        let manager = app.add_model(RemoteServerManager::new);
        let host_id = HostId::new("missing-host".to_string());
        let pending = manager.update(&mut app, |manager, _ctx| {
            manager.start_ripgrep_search(
                &host_id,
                RipgrepSearchParams {
                    pattern: "needle".to_string(),
                    roots: vec![StandardizedPath::try_new("/repo").unwrap()],
                    ignore_case: false,
                    multiline: false,
                    max_matches: 100,
                },
            )
        });

        assert!(matches!(
            pending.result().await,
            Err(HostRequestError::AllSessionsDisconnected)
        ));
    });
}
