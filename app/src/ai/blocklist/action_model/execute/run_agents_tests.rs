use ai::agent::action::{RunAgentsAgentRunConfig, RunAgentsExecutionMode};
use ai::agent::action_result::RunAgentsAgentOutcomeKind;
use warp_core::features::FeatureFlag;

use super::{
    build_agent_outcomes, compose_run_agents_child_prompt, run_agents_to_start_agent_mode,
};
use crate::ai::agent::StartAgentExecutionMode;

#[test]
fn compose_child_prompt_joins_non_empty_parts() {
    assert_eq!(
        compose_run_agents_child_prompt("shared", "specialized"),
        "shared\n\nspecialized"
    );
}

#[test]
fn compose_child_prompt_does_not_add_blank_separators() {
    assert_eq!(compose_run_agents_child_prompt("shared", "  "), "shared");
    assert_eq!(
        compose_run_agents_child_prompt("", "specialized"),
        "specialized"
    );
    assert_eq!(compose_run_agents_child_prompt("", ""), "");
}

#[test]
fn local_codex_batch_maps_to_local_codex_children() {
    let _local_codex = FeatureFlag::LocalClaudeCodexChildHarnesses.override_enabled(true);
    assert_eq!(
        run_agents_to_start_agent_mode(&RunAgentsExecutionMode::Local, "codex", "provider/model"),
        Ok(StartAgentExecutionMode::Local {
            harness_type: Some("codex".to_string()),
            model_id: Some("provider/model".to_string()),
        })
    );
}

#[test]
fn local_oz_batch_maps_to_embedded_children() {
    assert_eq!(
        run_agents_to_start_agent_mode(&RunAgentsExecutionMode::Local, "oz", ""),
        Ok(StartAgentExecutionMode::local_with_defaults())
    );
}

#[test]
fn unsupported_local_harness_is_rejected() {
    assert_eq!(
        run_agents_to_start_agent_mode(&RunAgentsExecutionMode::Local, "future-cli", ""),
        Err("Unsupported local child harness 'future-cli'.".to_string())
    );
}

#[test]
fn child_outcomes_preserve_request_order() {
    let configs = vec![
        RunAgentsAgentRunConfig {
            name: "first".to_string(),
            prompt: String::new(),
            title: String::new(),
        },
        RunAgentsAgentRunConfig {
            name: "second".to_string(),
            prompt: String::new(),
            title: String::new(),
        },
    ];
    let outcomes = build_agent_outcomes(
        &configs,
        vec![
            RunAgentsAgentOutcomeKind::Launched {
                agent_id: "one".to_string(),
            },
            RunAgentsAgentOutcomeKind::Failed {
                error: "failed".to_string(),
            },
        ],
    );

    assert_eq!(outcomes[0].name, "first");
    assert_eq!(outcomes[1].name, "second");
    assert_eq!(
        outcomes[0].kind,
        RunAgentsAgentOutcomeKind::Launched {
            agent_id: "one".to_string(),
        }
    );
    assert_eq!(
        outcomes[1].kind,
        RunAgentsAgentOutcomeKind::Failed {
            error: "failed".to_string(),
        }
    );
}
