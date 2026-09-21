use std::collections::HashMap;

use async_compat::CompatExt as _;
use futures::FutureExt as _;
use rmcp::ServiceExt as _;
use warpui::{App, Entity};

use crate::ai::agent::api::RequestParams;
use crate::ai::mcp::{
    FileBasedMCPManager, FileMCPWatcher, FileMCPWatcherEvent, MCPServerState,
    ParsedTemplatableMCPServerResult, TemplatableMCPServerManager,
};
use crate::settings::AISettings;

use axum::Router;
use axum::routing::post;
use http::header::{HeaderMap, HeaderValue, WWW_AUTHENTICATE};
use reqwest::StatusCode;
use warp_core::features::FeatureFlag;

use super::{
    Transport, determine_transport, has_caller_supplied_credential, is_oauth_challenge,
    send_initialize_request,
};

struct DispatchProbe;

impl Entity for DispatchProbe {
    type Event = ();
}

#[test]
fn first_dispatch_snapshots_tools_after_initial_scan_not_before_wait() {
    let _file_flag = FeatureFlag::FileBasedMcp.override_enabled(true);
    let _grouped_flag = FeatureFlag::MCPGroupedServerContext.override_enabled(true);
    App::test((), |mut app| async move {
        let watcher = app.add_singleton_model(|_| FileMCPWatcher::new_inert());
        app.add_singleton_model(AISettings::new_with_defaults);
        app.add_singleton_model(FileBasedMCPManager::new);
        let servers = app.add_singleton_model(|_| TemplatableMCPServerManager::default());
        let probe = app.add_model(|_| DispatchProbe);
        let mut prepared = Box::pin(probe.update(&mut app, |_, ctx| {
            RequestParams::new_for_test(Vec::new(), Vec::new()).prepare_for_dispatch(ctx)
        }));
        assert!(prepared.as_mut().now_or_never().is_none());

        let installation = ParsedTemplatableMCPServerResult::from_user_json(
            r#"{"late-server":{"command":"synthetic-command"}}"#,
        )
        .unwrap()
        .pop()
        .unwrap()
        .templatable_mcp_server_installation
        .unwrap();
        let uuid = installation.uuid();
        let (client_io, server_io) = tokio::io::duplex(64);
        let service = async move {
            rmcp::service::serve_directly(
                rmcp::model::ClientInfo::default().into_dyn(),
                client_io,
                None,
            )
        }
        .compat()
        .await;
        let info = mcp::TemplatableMCPServerInfo::new(
            "late-server".to_string(),
            service,
            Vec::new(),
            vec![rmcp::model::Tool::new(
                "late-tool",
                "synthetic test tool",
                serde_json::Map::new(),
            )],
            uuid,
            None,
            false,
        );
        servers.update(&mut app, |manager, ctx| {
            manager.locally_installed_servers.insert(uuid, installation);
            manager.active_servers.insert(uuid, info);
            manager.change_server_state(uuid, MCPServerState::Running, ctx);
        });
        assert!(
            prepared.as_mut().now_or_never().is_none(),
            "scan must finish too"
        );
        watcher.update(&mut app, |_, ctx| {
            ctx.emit(FileMCPWatcherEvent::InitialGlobalScanComplete)
        });
        let params = prepared.await.unwrap();
        let context = params.mcp_context.unwrap();
        assert_eq!(context.servers.len(), 1);
        assert_eq!(context.servers[0].id, uuid.to_string());
        assert_eq!(context.servers[0].tools[0].name, "late-tool");

        let next = probe
            .update(&mut app, |_, ctx| {
                RequestParams::new_for_test(Vec::new(), Vec::new()).prepare_for_dispatch(ctx)
            })
            .now_or_never()
            .expect("later dispatch must not wait")
            .unwrap();
        assert_eq!(
            next.mcp_context.unwrap().servers[0].tools[0].name,
            "late-tool"
        );
        let info = servers.update(&mut app, |manager, _| {
            manager.active_servers.remove(&uuid).unwrap()
        });
        info.shutdown().compat().await.unwrap();
        drop(server_io);
    });
}

#[test]
fn dispatch_with_file_mcp_disabled_is_immediate_without_scan_singleton() {
    let _file_flag = FeatureFlag::FileBasedMcp.override_enabled(false);
    let _grouped_flag = FeatureFlag::MCPGroupedServerContext.override_enabled(true);
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());
        let probe = app.add_model(|_| DispatchProbe);
        let params = probe
            .update(&mut app, |_, ctx| {
                RequestParams::new_for_test(Vec::new(), Vec::new()).prepare_for_dispatch(ctx)
            })
            .now_or_never()
            .expect("disabled feature must be immediate")
            .unwrap();
        assert!(params.mcp_context.is_none());
    });
}

#[test]
fn preparation_returns_model_dropped_when_request_disappears_during_scan() {
    let _file_flag = FeatureFlag::FileBasedMcp.override_enabled(true);
    App::test((), |mut app| async move {
        let watcher = app.add_singleton_model(|_| FileMCPWatcher::new_inert());
        app.add_singleton_model(AISettings::new_with_defaults);
        app.add_singleton_model(FileBasedMCPManager::new);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());
        let probe = app.add_model(|_| DispatchProbe);
        let mut prepared = Box::pin(probe.update(&mut app, |_, ctx| {
            RequestParams::new_for_test(Vec::new(), Vec::new()).prepare_for_dispatch(ctx)
        }));
        assert!(prepared.as_mut().now_or_never().is_none());
        app.update(|_| drop(probe));
        watcher.update(&mut app, |_, ctx| {
            ctx.emit(FileMCPWatcherEvent::InitialGlobalScanComplete)
        });
        assert!(matches!(prepared.await, Err(warpui::ModelDropped)));
    });
}

struct PreflightServer {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl PreflightServer {
    async fn start(status: StatusCode, challenges: &[&str]) -> Self {
        let mut response_headers = HeaderMap::new();
        for challenge in challenges {
            response_headers.append(WWW_AUTHENTICATE, HeaderValue::from_str(challenge).unwrap());
        }
        Self::start_with_headers(status, response_headers).await
    }

    async fn start_with_headers(status: StatusCode, response_headers: HeaderMap) -> Self {
        let router = Router::new().route(
            "/mcp",
            post(move || {
                let headers = response_headers.clone();
                async move { (status, headers, "") }
            }),
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { url, task }
    }
}

impl Drop for PreflightServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[test]
fn configured_credentials_require_nonempty_recognized_headers() {
    for name in ["Authorization", "AUTHORIZATION", "X-API-Key", "Api-Key"] {
        assert!(has_caller_supplied_credential(&HashMap::from([(
            name.to_string(),
            "synthetic-token".to_string()
        ),])));
        for empty in ["", " \t "] {
            assert!(!has_caller_supplied_credential(&HashMap::from([(
                name.to_string(),
                empty.to_string()
            ),])));
        }
    }
    assert!(!has_caller_supplied_credential(&HashMap::from([(
        "X-Request-Id".to_string(),
        "not-a-credential".to_string()
    ),])));
    assert!(!has_caller_supplied_credential(&HashMap::new()));
}

#[test]
fn oauth_challenge_matches_parameter_names_not_quoted_descriptions() {
    for challenge in [
        r#"Bearer resource_metadata="https://mcp.example.test/.well-known/oauth-protected-resource""#,
        r#"Bearer realm="example", RESOURCE_METADATA = "https://mcp.example.test/metadata""#,
        r#"Basic realm="legacy", Bearer resource_metadata="https://mcp.example.test/metadata""#,
    ] {
        assert!(is_oauth_challenge(challenge.as_bytes()), "{challenge}");
    }
    for challenge in [
        "",
        "Bearer",
        r#"Bearer error="invalid_token""#,
        r#"Bearer error_description="missing resource_metadata=value""#,
        r#"Bearer error_description="escaped \"resource_metadata=value\"""#,
        r#"Bearer error_description="unterminated resource_metadata=value"#,
        r#"Bearer other_resource_metadata="https://mcp.example.test/metadata""#,
    ] {
        assert!(!is_oauth_challenge(challenge.as_bytes()), "{challenge}");
    }
}

#[tokio::test]
async fn bare_401_with_configured_credentials_reports_rejection_without_oauth() {
    let _flag = FeatureFlag::McpOauth.override_enabled(true);
    for challenges in [vec![], vec![r#"Bearer error="invalid_token""#]] {
        let server = PreflightServer::start(StatusCode::UNAUTHORIZED, &challenges).await;
        for name in ["Authorization", "X-API-Key", "Api-Key"] {
            let headers = HashMap::from([(name.to_string(), "synthetic-secret-value".to_string())]);
            let error = determine_transport("test-server".to_string(), &server.url, &headers, None)
                .await
                .err()
                .expect("credential rejection must fail startup")
                .to_string();
            assert!(
                error.contains("rejected the configured credentials (HTTP 401)"),
                "{error}"
            );
            assert!(!error.contains("no authentication context"), "{error}");
            assert!(
                !error.contains("synthetic-secret-value"),
                "credentials must not enter diagnostics"
            );
        }
    }
}

#[tokio::test]
async fn all_challenges_are_preserved_and_genuine_oauth_still_wins() {
    let _flag = FeatureFlag::McpOauth.override_enabled(true);
    let challenges = [
        r#"Basic realm="legacy""#,
        r#"Bearer resource_metadata="https://mcp.example.test/metadata""#,
    ];
    let server = PreflightServer::start(StatusCode::UNAUTHORIZED, &challenges).await;
    let headers = HashMap::from([(
        "Authorization".to_string(),
        "stale-synthetic-token".to_string(),
    )]);
    let response = send_initialize_request(&server.url, &headers, None)
        .await
        .unwrap();
    assert_eq!(
        response.www_authenticate,
        challenges.map(|challenge| HeaderValue::from_str(challenge).unwrap()),
    );
    let error = determine_transport("test-server".to_string(), &server.url, &headers, None)
        .await
        .err()
        .unwrap()
        .to_string();
    // Stop at the auth-context guard: this proves OAuth routing without opening a browser or callback server.
    assert!(error.contains("no authentication context"), "{error}");
    assert!(
        !error.contains("rejected the configured credentials"),
        "{error}"
    );
}

#[tokio::test]
async fn opaque_auth_challenge_bytes_preserve_oauth_discrimination() {
    let _flag = FeatureFlag::McpOauth.override_enabled(true);
    let cases: &[(&[u8], bool)] = &[
        (
            r#"Bearer realm="café", resource_metadata="https://mcp.example.test/metadata""#
                .as_bytes(),
            true,
        ),
        (
            b"Bearer realm=\"caf\xe9\", resource_metadata=\"https://mcp.example.test/metadata\"",
            true,
        ),
        (
            b"Bearer error_description=\"caf\xe9 missing resource_metadata=value\"",
            false,
        ),
    ];
    let headers = HashMap::from([(
        "Authorization".to_string(),
        "synthetic-secret-value".to_string(),
    )]);
    for &(raw_challenge, expects_oauth) in cases {
        let challenge = HeaderValue::from_bytes(raw_challenge).unwrap();
        assert!(
            challenge.to_str().is_err(),
            "fixture must exercise opaque header bytes"
        );
        let mut response_headers = HeaderMap::new();
        response_headers.append(WWW_AUTHENTICATE, challenge);
        let server =
            PreflightServer::start_with_headers(StatusCode::UNAUTHORIZED, response_headers).await;
        let response = send_initialize_request(&server.url, &headers, None)
            .await
            .unwrap();
        assert_eq!(response.www_authenticate.len(), 1);
        assert_eq!(response.www_authenticate[0].as_bytes(), raw_challenge);
        let error = determine_transport("test-server".to_string(), &server.url, &headers, None)
            .await
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            error.contains("no authentication context"),
            expects_oauth,
            "{error}"
        );
        assert_eq!(
            error.contains("rejected the configured credentials"),
            !expects_oauth,
            "{error}"
        );
        assert!(
            !error.contains("synthetic-secret-value"),
            "credentials must not enter diagnostics"
        );
    }
}

#[tokio::test]
async fn bare_401_without_credentials_preserves_oauth_behavior() {
    let _flag = FeatureFlag::McpOauth.override_enabled(true);
    let server = PreflightServer::start(StatusCode::UNAUTHORIZED, &[]).await;
    let error = determine_transport(
        "test-server".to_string(),
        &server.url,
        &HashMap::new(),
        None,
    )
    .await
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("no authentication context"), "{error}");
    assert!(
        !error.contains("rejected the configured credentials"),
        "{error}"
    );
}

#[tokio::test]
async fn quoted_resource_metadata_does_not_start_oauth_after_credential_rejection() {
    let _flag = FeatureFlag::McpOauth.override_enabled(true);
    let server = PreflightServer::start(
        StatusCode::UNAUTHORIZED,
        &[r#"Bearer error_description="missing resource_metadata=value""#],
    )
    .await;
    let headers = HashMap::from([("Authorization".to_string(), "synthetic-token".to_string())]);
    let error = determine_transport("test-server".to_string(), &server.url, &headers, None)
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("rejected the configured credentials"),
        "{error}"
    );
}

#[tokio::test]
async fn successful_and_legacy_sse_transport_selection_are_unchanged() {
    for status in [
        StatusCode::OK,
        StatusCode::NOT_FOUND,
        StatusCode::METHOD_NOT_ALLOWED,
    ] {
        let server = PreflightServer::start(status, &[]).await;
        let transport = determine_transport(
            "test-server".to_string(),
            &server.url,
            &HashMap::new(),
            None,
        )
        .await
        .unwrap();
        match status {
            StatusCode::OK => assert!(matches!(transport, Transport::Http(None))),
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => {
                assert!(matches!(transport, Transport::Sse(None)));
            }
            _ => unreachable!("test only covers HTTP and legacy SSE statuses"),
        }
    }
}
