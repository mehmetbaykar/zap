//! `RunShellCommand` adaptation.
//!
//! Corresponds to `api::message::tool_call::Tool::RunShellCommand` in warp,
//! and after execution the result is `ToolCallResultType::RunShellCommand(RunShellCommandResult)`.

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use warp_multi_agent_api as api;

use super::OpenAiTool;

#[derive(Debug, Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    is_read_only: bool,
    #[serde(default)]
    uses_pager: bool,
    #[serde(default)]
    is_risky: bool,
    /// `None` (default / true) = return after the command completes; `Some(false)` = return immediately after starting
    /// with a LongRunningCommandSnapshot, after which read/write_to_long_running_*
    /// tools can continue interacting (suitable for continuously running commands like a dev server / tail -f).
    #[serde(default)]
    wait_until_complete: Option<bool>,
}

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "The shell command to execute (the full command line)."
            },
            "is_read_only": {
                "type": "boolean",
                "description": "Whether the command only reads information and doesn't modify the filesystem/external state (when true, no user confirmation is needed).",
                "default": false
            },
            "uses_pager": {
                "type": "boolean",
                "description": "Whether the command triggers a pager (less/more, etc.). Recommended false; you can append something like | cat to avoid blocking.",
                "default": false
            },
            "is_risky": {
                "type": "boolean",
                "description": "Whether the command is dangerous (rm -rf, changing global config, etc.). Set true to make the user confirm more prominently.",
                "default": false
            },
            "wait_until_complete": {
                "type": "boolean",
                "description": "Defaults to true (return only after the command ends, suitable for one-off commands). Commands that don't exit naturally, like a dev server / background process / tail -f / interactive REPL, must be set to false, otherwise the current turn hangs forever waiting for a result. After setting false, it returns a LongRunningCommandSnapshot immediately, and later turns continue interacting with read/write_to_long_running_shell_command.",
                "default": true
            }
        },
        "required": ["command"],
        "additionalProperties": false
    })
}

fn from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    use api::message::tool_call::run_shell_command::WaitUntilCompleteValue;
    let parsed: Args = serde_json::from_str(args)?;
    // When None, explicitly default to true (return only after the command completes), to avoid the controller's implicit default behavior
    // being ambiguous across different warp versions/paths. If the model wants long-running mode it must explicitly pass false.
    let wait_until_complete_value = Some(WaitUntilCompleteValue::WaitUntilComplete(
        parsed.wait_until_complete.unwrap_or(true),
    ));
    Ok(api::message::tool_call::Tool::RunShellCommand(
        api::message::tool_call::RunShellCommand {
            command: parsed.command,
            is_read_only: parsed.is_read_only,
            uses_pager: parsed.uses_pager,
            is_risky: parsed.is_risky,
            citations: vec![],
            wait_until_complete_value,
            risk_category: 0,
        },
    ))
}

fn result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    use api::run_shell_command_result::Result as ShellR;
    let r = match result {
        R::RunShellCommand(r) => r,
        _ => return None,
    };
    let value = match &r.result {
        Some(ShellR::CommandFinished(f)) => json!({
            "status": "completed",
            "command": r.command,
            "exit_code": f.exit_code,
            "output": f.output,
        }),
        // Long-running command: started but not yet finished. Expose the snapshot to the model so the model can
        // decide whether to continue reading (read_shell_command_output) or writing (write_to_long_running_*).
        Some(ShellR::LongRunningCommandSnapshot(s)) => json!({
            "status": "running",
            "command": r.command,
            "command_id": s.command_id,
            "output": s.output,
            "is_alt_screen_active": s.is_alt_screen_active,
        }),
        Some(ShellR::PermissionDenied(_)) => json!({
            "status": "permission_denied",
            "command": r.command,
        }),
        None => json!({ "status": "cancelled", "command": r.command }),
    };
    Some(value)
}

pub static RUN_SHELL_COMMAND: OpenAiTool = OpenAiTool {
    name: "run_shell_command",
    description: include_str!("../prompts/tool_descriptions/run_shell_command.md"),
    parameters,
    from_args,
    result_to_json,
};
