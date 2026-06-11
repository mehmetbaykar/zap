//! Prompt cache serialization stability test suite (corresponding to docs P1-8 / P1-9 / P1-13).
//!
//! The Anthropic docs explicitly warn:
//! > Verify that the keys in your `tool_use` content blocks have stable
//! > ordering as some languages (for example, Swift, Go) randomize key order
//! > during JSON conversion, breaking caches
//!
//! This means any `serde_json::Value` produced on the Rust side **must**:
//!   1. be byte-equal across calls for the same input (deterministic)
//!   2. not depend on `HashMap` iteration order
//!   3. not depend on external state (timestamps, randomness, PID, etc.)
//!
//! This test suite is Zap's "anti-regression guardrail" — any later change to the prompt
//! construction path that breaks byte-level stability will fail an assertion here.

use crate::ai::agent::{MCPContext, MCPServer};
use api::message;
use warp_multi_agent_api as api;

use super::chat_stream;
use super::tools;

// ---------------------------------------------------------------------------
// P1-8: tool schema field order stability
// ---------------------------------------------------------------------------

/// Calls `(parameters)()` twice for each tool in `REGISTRY`, asserting byte-equal.
///
/// Risk: if the enum / oneof embedded in a tool schema uses `HashMap<String, Schema>`
/// when converting to Value, the order gets scrambled. The `serde_json::Map` produced by `json!({...})` literals preserves
/// **insertion order** by default (`preserve_order` is on by default, see Cargo.toml), so
/// the literally hardcoded key order is stable across calls. This test guards that invariant.
#[test]
fn registry_tool_schemas_are_deterministic() {
    for tool in tools::REGISTRY {
        let s1 = (tool.parameters)();
        let s2 = (tool.parameters)();
        let j1 = serde_json::to_string(&s1).unwrap();
        let j2 = serde_json::to_string(&s2).unwrap();
        assert_eq!(
            j1, j2,
            "tool `{}`'s schema must be byte-equal across calls (prerequisite for a prompt cache hit)",
            tool.name
        );
    }
}

/// Calls each tool in `REGISTRY` repeatedly 50 times, asserting all calls produce byte-equal output.
/// Prevents occasional HashMap iteration order drift (running only twice might coincidentally match).
#[test]
fn registry_tool_schemas_stable_under_repetition() {
    for tool in tools::REGISTRY {
        let baseline = serde_json::to_string(&(tool.parameters)()).unwrap();
        for i in 0..50 {
            let candidate = serde_json::to_string(&(tool.parameters)()).unwrap();
            assert_eq!(
                baseline, candidate,
                "tool `{}`'s call #{i} output differs from the baseline (HashMap order drift may exist)",
                tool.name
            );
        }
    }
}

/// `tools::REGISTRY`'s own order is static, but verify it anyway:
/// iterating multiple times within the same process yields the same (name, description) sequence.
#[test]
fn registry_iteration_order_is_stable() {
    let names1: Vec<&str> = tools::REGISTRY.iter().map(|t| t.name).collect();
    let names2: Vec<&str> = tools::REGISTRY.iter().map(|t| t.name).collect();
    assert_eq!(names1, names2);
}

// ---------------------------------------------------------------------------
// P1-9: serialize_outgoing_tool_call historical replay stability
// ---------------------------------------------------------------------------

/// Simulates a Grep tool call, verifying that two serializations produce byte-equal output.
/// `serialize_outgoing_tool_call` reruns on every build_chat_request,
/// converting historical turns' ToolCall into (name, args Value). Any HashMap / time-related
/// instability invalidates the cache for the latter half of the messages section.
///
/// Grep is chosen because its fields are the simplest (`queries: Vec<String>`, `path: String`),
/// not depending on any prost implicit default fields.
#[test]
fn serialize_grep_tool_call_is_deterministic() {
    let tc = message::ToolCall {
        tool_call_id: "call-grep-1".to_owned(),
        tool: Some(message::tool_call::Tool::Grep(message::tool_call::Grep {
            queries: vec!["fn main".to_owned(), "Result<".to_owned()],
            path: "src/".to_owned(),
        })),
    };

    let (n1, v1) = chat_stream::serialize_outgoing_tool_call_for_test(&tc, None, "");
    let (n2, v2) = chat_stream::serialize_outgoing_tool_call_for_test(&tc, None, "");
    assert_eq!(n1, n2, "tool name must be consistent");
    let j1 = serde_json::to_string(&v1).unwrap();
    let j2 = serde_json::to_string(&v2).unwrap();
    assert_eq!(
        j1, j2,
        "the same ToolCall is byte-equal across serializations"
    );
}

/// Grep `queries` is a `Vec<String>`, and its order must be stable (Vec is naturally stable, but this is a defensive assertion).
/// This reflects a larger rule: any Vec field within a user ToolCall must preserve the input order.
#[test]
fn serialize_grep_preserves_queries_order() {
    let tc = message::ToolCall {
        tool_call_id: "call-grep-2".to_owned(),
        tool: Some(message::tool_call::Tool::Grep(message::tool_call::Grep {
            queries: vec!["zzz".to_owned(), "aaa".to_owned()],
            path: ".".to_owned(),
        })),
    };
    let (_, v) = chat_stream::serialize_outgoing_tool_call_for_test(&tc, None, "");
    let s = serde_json::to_string(&v).unwrap();
    let pos_z = s.find("zzz").expect("queries should contain zzz");
    let pos_a = s.find("aaa").expect("queries should contain aaa");
    assert!(
        pos_z < pos_a,
        "Vec order must be preserved per the input (zzz first, aaa second)"
    );
}

/// An MCP tool call contains `prost_types::Struct`; verify serialization is stable.
/// `prost_types::Struct.fields` uses a `BTreeMap` internally, which is stable on its own; we cover it here to confirm.
#[test]
fn serialize_mcp_tool_call_is_deterministic() {
    use prost_types::{value::Kind, Struct, Value as ProstValue};
    use std::collections::BTreeMap;

    let mut fields = BTreeMap::new();
    fields.insert(
        "key_z".to_owned(),
        ProstValue {
            kind: Some(Kind::StringValue("v_z".to_owned())),
        },
    );
    fields.insert(
        "key_a".to_owned(),
        ProstValue {
            kind: Some(Kind::NumberValue(42.0)),
        },
    );

    let server_id = "srv-uuid-1".to_owned();
    let tc = message::ToolCall {
        tool_call_id: "call-mcp-1".to_owned(),
        tool: Some(message::tool_call::Tool::CallMcpTool(
            message::tool_call::CallMcpTool {
                name: "echo".to_owned(),
                args: Some(Struct { fields }),
                server_id: server_id.clone(),
            },
        )),
    };

    // construct an mcp_context so sanitize_server_name can look up the server name
    let ctx = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![MCPServer {
            id: server_id.clone(),
            name: "my-server".to_owned(),
            description: String::new(),
            resources: vec![],
            tools: vec![],
        }],
    };

    let (n1, v1) = chat_stream::serialize_outgoing_tool_call_for_test(&tc, Some(&ctx), "");
    let (n2, v2) = chat_stream::serialize_outgoing_tool_call_for_test(&tc, Some(&ctx), "");
    assert_eq!(n1, n2);
    let j1 = serde_json::to_string(&v1).unwrap();
    let j2 = serde_json::to_string(&v2).unwrap();
    assert_eq!(j1, j2);
    // BTreeMap should output by key lexicographic order (key_a before key_z)
    let pos_a = j1.find("key_a").expect("should contain key_a");
    let pos_z = j1.find("key_z").expect("should contain key_z");
    assert!(
        pos_a < pos_z,
        "prost_types::Struct should follow BTreeMap key lexicographic order"
    );
}

// ---------------------------------------------------------------------------
// P1-13: build_tools_array overall stability (in conjunction with P0-3's MCP ordering)
// ---------------------------------------------------------------------------

/// End-to-end assertion: running the tools-array assembly twice for the same `(REGISTRY + same mcp_context)`
/// yields a byte-equal string. This covers the key stability constraint of the tools array in the prompt
/// (Anthropic docs: any change to tool definitions → all caches invalidated).
///
/// It doesn't call `build_tools_array(params: &RequestParams)` directly because `RequestParams`
/// has too many fields, raising the construction barrier; here it replicates the core assembly logic for the REGISTRY and mcp parts.
#[test]
fn full_tools_array_serialization_is_stable() {
    let assemble = || -> String {
        let mut buf = String::new();
        // built-in tools (REGISTRY iteration order is static)
        for t in tools::REGISTRY {
            buf.push_str(t.name);
            buf.push('|');
            buf.push_str(t.description);
            buf.push('|');
            let schema = (t.parameters)();
            buf.push_str(&serde_json::to_string(&schema).unwrap());
            buf.push('\n');
        }
        // MCP tools (already sorted inside build_mcp_tool_defs; empty when there's no ctx)
        buf
    };
    let a = assemble();
    let b = assemble();
    assert_eq!(a.len(), b.len());
    assert_eq!(
        a, b,
        "the tools array serialization result must be byte-equal across calls"
    );
}

/// End-to-end assembly stability with an MCP server (connecting to the P0-3 ordering guarantee).
#[test]
fn full_tools_array_with_mcp_is_stable() {
    use rmcp::model::{AnnotateAble, RawResource, Tool as McpTool};
    use serde_json::json;
    use std::sync::Arc;

    let schema_obj = json!({
        "type": "object",
        "properties": { "x": { "type": "string" } }
    })
    .as_object()
    .unwrap()
    .clone();

    let server_a = MCPServer {
        id: "id-a".to_owned(),
        name: "server-a".to_owned(),
        description: String::new(),
        resources: vec![RawResource::new("file:///x.txt", "X").no_annotation()],
        tools: vec![
            McpTool::new("zeta", "Z desc", Arc::new(schema_obj.clone())),
            McpTool::new("alpha", "A desc", Arc::new(schema_obj.clone())),
        ],
    };
    let ctx1 = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![server_a.clone()],
    };
    // reconstruct the same ctx once more (the servers Vec order is identical):
    let ctx2 = MCPContext {
        #[allow(deprecated)]
        resources: vec![],
        #[allow(deprecated)]
        tools: vec![],
        servers: vec![server_a],
    };

    let assemble = |ctx: &MCPContext| -> String {
        let mut buf = String::new();
        for t in tools::REGISTRY {
            buf.push_str(t.name);
            buf.push('|');
            buf.push_str(t.description);
            buf.push('|');
            let schema = (t.parameters)();
            buf.push_str(&serde_json::to_string(&schema).unwrap());
            buf.push('\n');
        }
        for (name, desc, schema) in tools::mcp::build_mcp_tool_defs(ctx) {
            buf.push_str(&name);
            buf.push('|');
            buf.push_str(&desc);
            buf.push('|');
            buf.push_str(&serde_json::to_string(&schema).unwrap());
            buf.push('\n');
        }
        buf
    };

    let a = assemble(&ctx1);
    let b = assemble(&ctx2);
    assert_eq!(
        a, b,
        "the tools array with MCP must be byte-equal across calls"
    );
    // verify MCP tools follow function_name lexicographic order (alpha before zeta)
    let pos_alpha = a
        .find("mcp__server-a__alpha")
        .expect("should contain alpha");
    let pos_zeta = a.find("mcp__server-a__zeta").expect("should contain zeta");
    assert!(
        pos_alpha < pos_zeta,
        "the P0-3 ordering guarantees alpha < zeta"
    );
}
