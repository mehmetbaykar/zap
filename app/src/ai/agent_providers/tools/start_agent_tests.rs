use serde_json::json;
use warp_multi_agent_api as api;

use super::*;

fn from_args_v2(args: &str) -> api::StartAgentV2 {
    match (START_AGENT.from_args)(args).expect("from_args should parse") {
        api::message::tool_call::Tool::StartAgentV2(v2) => v2,
        other => panic!("expected StartAgentV2, got {other:?}"),
    }
}

#[test]
fn from_args_defaults_to_native_local_child() {
    let v2 = from_args_v2(r#"{"name":"Test Fixer","prompt":"fix the tests"}"#);
    assert_eq!(v2.name, "Test Fixer");
    assert_eq!(v2.prompt, "fix the tests");
    assert!(v2.lifecycle_subscription.is_none());
    let mode = v2.execution_mode.and_then(|m| m.mode).expect("mode set");
    match mode {
        api::start_agent_v2::execution_mode::Mode::Local(local) => {
            assert!(local.harness.is_none());
        }
        other => panic!("expected Local mode, got {other:?}"),
    }
}

#[test]
fn from_args_passes_harness_through() {
    let v2 = from_args_v2(r#"{"name":"a","prompt":"b","harness":"claude"}"#);
    let mode = v2.execution_mode.and_then(|m| m.mode).expect("mode set");
    match mode {
        api::start_agent_v2::execution_mode::Mode::Local(local) => {
            assert_eq!(local.harness.expect("harness set").r#type, "claude");
        }
        other => panic!("expected Local mode, got {other:?}"),
    }
}

#[test]
fn from_args_whitespace_harness_is_native() {
    let v2 = from_args_v2(r#"{"name":"a","prompt":"b","harness":"  "}"#);
    let mode = v2.execution_mode.and_then(|m| m.mode).expect("mode set");
    match mode {
        api::start_agent_v2::execution_mode::Mode::Local(local) => {
            assert!(local.harness.is_none());
        }
        other => panic!("expected Local mode, got {other:?}"),
    }
}

#[test]
fn from_args_rejects_missing_prompt() {
    assert!((START_AGENT.from_args)(r#"{"name":"a"}"#).is_err());
}

#[test]
fn result_to_json_success_v2() {
    let result = api::message::tool_call_result::Result::StartAgentV2(api::StartAgentV2Result {
        result: Some(api::start_agent_v2_result::Result::Success(
            api::start_agent_v2_result::Success {
                agent_id: "agent-42".to_string(),
            },
        )),
    });
    let value = (START_AGENT.result_to_json)(&result).expect("should serialize");
    assert_eq!(
        value,
        json!({ "status": "started", "agent_id": "agent-42" })
    );
}

#[test]
fn result_to_json_error_v1() {
    let result = api::message::tool_call_result::Result::StartAgent(api::StartAgentResult {
        result: Some(api::start_agent_result::Result::Error(
            api::start_agent_result::Error {
                error: "no such harness".to_string(),
            },
        )),
    });
    let value = (START_AGENT.result_to_json)(&result).expect("should serialize");
    assert_eq!(
        value,
        json!({ "status": "error", "error": "no such harness" })
    );
}

#[test]
fn result_to_json_ignores_other_variants() {
    let result = api::message::tool_call_result::Result::Server(
        api::message::tool_call_result::ServerResult {
            serialized_result: String::new(),
        },
    );
    assert!((START_AGENT.result_to_json)(&result).is_none());
}
