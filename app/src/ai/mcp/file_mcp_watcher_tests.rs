use std::collections::HashSet;
use std::env;
use std::path::PathBuf;

use futures::stream::AbortHandle;
use warpui::{App, Entity, ModelHandle};

use super::{
    FileMCPConfigDiagnosticKind, FileMCPConfigParseOutcome, FileMCPWatcher, FileMCPWatcherEvent,
    InFlightParse, parse_mcp_config_file, substitute_env_vars,
};
use crate::ai::mcp::MCPProvider;

fn cleanup_env_vars(vars: &[&str]) {
    for var in vars {
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var(var) };
    }
}

#[test]
fn test_substitute_env_vars_success() {
    let test_vars = ["FOO", "BAZ", "REPEATED"];

    // Setup environment variables
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { env::set_var("FOO", "bar") };
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { env::set_var("BAZ", "qux") };
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { env::set_var("REPEATED", "value") };

    // Test 1: Single variable substitution
    let input = r#"{"key": "${FOO}"}"#;
    let result = substitute_env_vars(input).expect("Single variable substitution should succeed");
    assert_eq!(
        result, r#"{"key": "bar"}"#,
        "Single variable FOO should be replaced with 'bar'"
    );

    // Test 2: Multiple different variables
    let input = r#"{"key": "${FOO}", "other": "${BAZ}"}"#;
    let result = substitute_env_vars(input).expect("Multiple variable substitution should succeed");
    assert_eq!(
        result, r#"{"key": "bar", "other": "qux"}"#,
        "Multiple variables FOO and BAZ should be replaced"
    );

    // Test 3: Multiple occurrences of same variable
    let input = r#"{"a": "${REPEATED}", "b": "${REPEATED}", "c": "prefix_${REPEATED}_suffix"}"#;
    let result = substitute_env_vars(input).expect("Repeated variable substitution should succeed");
    assert_eq!(
        result, r#"{"a": "value", "b": "value", "c": "prefix_value_suffix"}"#,
        "All occurrences of REPEATED should be replaced with 'value', including within context"
    );

    // Cleanup
    cleanup_env_vars(&test_vars);
}

#[test]
fn test_substitute_env_vars_missing_or_empty() {
    // Test 1: Missing variable
    // Ensure MISSING_VAR is not set
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { env::remove_var("MISSING_VAR") };

    let input = r#"{"key": "${MISSING_VAR}"}"#;
    let result = substitute_env_vars(input);
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Missing or empty environment variable: MISSING_VAR"),
        "Error message should mention MISSING_VAR, got: {err_msg}"
    );

    // Test 2: Empty variable
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { env::set_var("EMPTY_VAR", "") };

    let input = r#"{"key": "${EMPTY_VAR}"}"#;
    let result = substitute_env_vars(input);
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Missing or empty environment variable: EMPTY_VAR"),
        "Error message should mention EMPTY_VAR, got: {err_msg}"
    );

    // Cleanup
    cleanup_env_vars(&["EMPTY_VAR"]);
}

#[test]
fn abort_config_parse_cancels_and_removes_inflight_task() {
    let config_path = PathBuf::from("/tmp/.mcp.json");
    let key = (config_path.clone(), MCPProvider::Zap);
    let (abort_handle, _abort_registration) = AbortHandle::new_pair();
    let observed_handle = abort_handle.clone();
    let mut watcher = FileMCPWatcher::new_inert();
    watcher.in_flight_parses.insert(
        key.clone(),
        InFlightParse {
            generation: 0,
            abort_handle,
        },
    );

    watcher.abort_config_parse(&config_path, MCPProvider::Zap);

    assert!(observed_handle.is_aborted());
    assert!(!watcher.in_flight_parses.contains_key(&key));
}

#[tokio::test]
async fn parse_outcomes_distinguish_missing_invalid_and_valid_configs() {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let path = directory.path().join(".mcp.json");

    assert!(matches!(
        parse_mcp_config_file(&path, MCPProvider::Zap).await,
        FileMCPConfigParseOutcome::Missing
    ));

    std::fs::write(&path, "{invalid").expect("invalid config should be written");
    match parse_mcp_config_file(&path, MCPProvider::Zap).await {
        FileMCPConfigParseOutcome::Error(diagnostic) => {
            assert_eq!(diagnostic.kind, FileMCPConfigDiagnosticKind::Parse);
        }
        _ => panic!("invalid JSON should produce a parse diagnostic"),
    }

    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("WARP_MCP_TEST_MISSING") };
    std::fs::write(
        &path,
        r#"{"mcpServers":{"test":{"command":"${WARP_MCP_TEST_MISSING}"}}}"#,
    )
    .expect("missing-env config should be written");
    match parse_mcp_config_file(&path, MCPProvider::Zap).await {
        FileMCPConfigParseOutcome::Error(diagnostic) => {
            assert_eq!(
                diagnostic.kind,
                FileMCPConfigDiagnosticKind::MissingEnvironmentVariable
            );
        }
        _ => panic!("missing env should produce a diagnostic"),
    }

    std::fs::write(
        &path,
        r#"{"mcpServers":{"test":{"command":"test-command"}}}"#,
    )
    .expect("valid config should be written");
    match parse_mcp_config_file(&path, MCPProvider::Zap).await {
        FileMCPConfigParseOutcome::Parsed(servers) => assert_eq!(servers.len(), 1),
        _ => panic!("valid config should produce one server"),
    }
}

#[derive(Default)]
struct ScanEvents {
    order: Vec<&'static str>,
    complete: Option<futures::channel::oneshot::Sender<()>>,
}

impl Entity for ScanEvents {
    type Event = ();
}

fn track_scan(
    app: &mut App,
    watcher: &ModelHandle<FileMCPWatcher>,
) -> (
    ModelHandle<ScanEvents>,
    futures::channel::oneshot::Receiver<()>,
) {
    let (tx, rx) = futures::channel::oneshot::channel();
    let events = app.add_model::<ScanEvents, _>(|ctx| {
        ctx.subscribe_to_model(watcher, |me, _, event, _| {
            me.order.push(match event {
                FileMCPWatcherEvent::ConfigParsed { .. } => "parsed",
                FileMCPWatcherEvent::ConfigRemoved { .. } => "removed",
                FileMCPWatcherEvent::ConfigError { .. } => "error",
                FileMCPWatcherEvent::InitialGlobalScanComplete => {
                    if let Some(tx) = me.complete.take() {
                        let _ = tx.send(());
                    }
                    "complete"
                }
            });
        });
        ScanEvents {
            order: Vec::new(),
            complete: Some(tx),
        }
    });
    (events, rx)
}

#[test]
fn initial_scan_waits_for_valid_missing_and_invalid_sources() {
    let directory = tempfile::tempdir().unwrap();
    let valid = directory.path().join("valid.json");
    let invalid = directory.path().join("invalid.json");
    let missing = directory.path().join("missing.json");
    let unreadable = directory.path().join("directory.json");
    std::fs::write(
        &valid,
        r#"{"mcpServers":{"test":{"command":"synthetic-command"}}}"#,
    )
    .unwrap();
    std::fs::write(&invalid, "{invalid").unwrap();
    std::fs::create_dir(&unreadable).unwrap();
    let paths = vec![valid, invalid, missing, unreadable];

    App::test((), |mut app| async move {
        let watcher = app.add_model(|_| FileMCPWatcher::new_inert());
        let (events, complete) = track_scan(&mut app, &watcher);
        watcher.update(&mut app, |watcher, ctx| {
            watcher.initial_global_pending = Some(
                paths
                    .iter()
                    .map(|path| (path.clone(), MCPProvider::Zap))
                    .collect(),
            );
            for path in paths {
                watcher.update_servers_from_config_file(
                    &path,
                    directory.path().to_path_buf(),
                    MCPProvider::Zap,
                    ctx,
                );
            }
        });
        complete.await.unwrap();
        events.read(&app, |events, _| {
            let order = &events.order;
            assert_eq!(order.len(), 5);
            assert_eq!(order.last(), Some(&"complete"));
            assert_eq!(order.iter().filter(|event| **event == "parsed").count(), 1);
            assert_eq!(order.iter().filter(|event| **event == "removed").count(), 1);
            assert_eq!(order.iter().filter(|event| **event == "error").count(), 2);
        });
        watcher.read(&app, |watcher, _| {
            assert!(watcher.initial_global_pending.is_none());
        });
    });
}

#[test]
fn superseded_parse_cannot_settle_source_or_remove_replacement() {
    let key = (
        PathBuf::from("/synthetic/home/.claude.json"),
        MCPProvider::Claude,
    );
    let mut watcher = FileMCPWatcher::new_inert();
    watcher.initial_global_pending = Some(HashSet::from([key.clone()]));
    watcher.in_flight_parses.insert(
        key.clone(),
        InFlightParse {
            generation: 2,
            abort_handle: AbortHandle::new_pair().0,
        },
    );
    assert!(!watcher.take_current_in_flight_parse(&key, 1));
    assert_eq!(watcher.in_flight_parses[&key].generation, 2);
    assert!(
        watcher
            .initial_global_pending
            .as_ref()
            .unwrap()
            .contains(&key)
    );
    assert!(watcher.take_current_in_flight_parse(&key, 2));
    assert!(!watcher.take_current_in_flight_parse(&key, 2));
}

#[test]
fn deleting_pending_source_settles_once_and_late_updates_do_not_reopen_scan() {
    App::test((), |mut app| async move {
        let key = (
            PathBuf::from("/synthetic/home/.codex/config.toml"),
            MCPProvider::Codex,
        );
        let watcher = app.add_model(|_| FileMCPWatcher::new_inert());
        let (events, complete) = track_scan(&mut app, &watcher);
        let abort_handle = AbortHandle::new_pair().0;
        watcher.update(&mut app, |watcher, ctx| {
            watcher.initial_global_pending = Some(HashSet::from([key.clone()]));
            watcher.in_flight_parses.insert(
                key.clone(),
                InFlightParse {
                    generation: 3,
                    abort_handle: abort_handle.clone(),
                },
            );
            watcher.remove_config(key.0.clone(), PathBuf::from("/synthetic/home"), key.1, ctx);
            assert!(!watcher.take_current_in_flight_parse(&key, 3));
            watcher.settle_pending_source(&key.0, key.1, ctx);
        });
        complete.await.unwrap();
        assert!(abort_handle.is_aborted());
        events.read(&app, |events, _| {
            assert_eq!(events.order, ["removed", "complete"]);
        });
    });
}

#[test]
fn empty_initial_scan_completes_once() {
    App::test((), |mut app| async move {
        let watcher = app.add_model(|_| FileMCPWatcher::new_inert());
        let (events, complete) = track_scan(&mut app, &watcher);
        watcher.update(&mut app, |watcher, ctx| {
            watcher.initial_global_pending = Some(HashSet::new());
            watcher.complete_initial_scan_if_settled(ctx);
            watcher.complete_initial_scan_if_settled(ctx);
        });
        complete.await.unwrap();
        events.read(&app, |events, _| {
            assert_eq!(events.order, ["complete"]);
        });
    });
}
