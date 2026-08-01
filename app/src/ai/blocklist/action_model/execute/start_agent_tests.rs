use super::{
    StartAgentOutcome, StartAgentRequest, invalid_local_child_harness_error,
    normalize_legacy_local_child_harness_command,
};
use crate::ai::agent::StartAgentExecutionMode;
use crate::ai::agent::conversation::AIConversationId;

#[test]
fn legacy_codex_command_becomes_a_local_codex_launch() {
    let (prompt, mode) = normalize_legacy_local_child_harness_command(
        "codex --dangerously-bypass-approvals-and-sandbox 'inspect the tests'".to_string(),
        StartAgentExecutionMode::local_with_defaults(),
    );

    assert_eq!(prompt, "inspect the tests");
    assert_eq!(
        mode,
        StartAgentExecutionMode::Local {
            harness_type: Some("codex".to_string()),
            model_id: None,
        }
    );
}

#[test]
fn non_legacy_prompt_remains_an_embedded_local_launch() {
    let original = "inspect the tests".to_string();
    let (prompt, mode) = normalize_legacy_local_child_harness_command(
        original.clone(),
        StartAgentExecutionMode::local_with_defaults(),
    );

    assert_eq!(prompt, original);
    assert_eq!(mode, StartAgentExecutionMode::local_with_defaults());
}

#[test]
fn unsupported_harness_error_preserves_the_requested_name() {
    assert_eq!(
        invalid_local_child_harness_error(" future-cli "),
        "Unsupported local child harness 'future-cli'."
    );
}

#[test]
fn local_request_completion_reports_the_conversation_identifier() {
    let (completion, receiver) = async_channel::bounded(1);
    let request = StartAgentRequest {
        name: "reviewer".to_string(),
        prompt: "review".to_string(),
        execution_mode: StartAgentExecutionMode::local_with_defaults(),
        lifecycle_subscription: None,
        parent_conversation_id: AIConversationId::new(),
        completion,
    };

    request.complete_started("local-conversation-id".to_string());

    assert_eq!(
        receiver.try_recv(),
        Ok(StartAgentOutcome::Started {
            agent_id: "local-conversation-id".to_string(),
        })
    );
}
