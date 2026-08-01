//! Local-only executor for `AIAgentActionType::StartAgent`.

use futures::future::BoxFuture;
use futures::FutureExt;
use shell_words::split as split_shell_words;
use warp_cli::agent::Harness;
use warp_core::execution_mode::AppExecutionMode;
use warpui::{Entity, EntityId, ModelContext, SingletonEntity};

use super::{ActionExecution, AnyActionExecution, ExecuteActionInput};
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent::{
    AIAgentActionResultType, AIAgentActionType, StartAgentExecutionMode, StartAgentResult,
};
use crate::ai::blocklist::permissions::BlocklistAIPermissions;
use crate::ai::local_harness_setup::local_harness_product_disabled_message;

/// Per-request outcome of a local child launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartAgentOutcome {
    Started { agent_id: String },
    Error(String),
}

/// App-local launch request passed from the action executor to the owning terminal pane.
///
/// The completion channel deliberately stays out of the serializable `ai` crate contract. The
/// pane completes it after creating the local child conversation and starting either Oz or a CLI
/// harness.
#[derive(Clone)]
pub struct StartAgentRequest {
    pub name: String,
    pub prompt: String,
    pub execution_mode: StartAgentExecutionMode,
    pub lifecycle_subscription: Option<Vec<String>>,
    pub parent_conversation_id: AIConversationId,
    completion: async_channel::Sender<StartAgentOutcome>,
}

impl StartAgentRequest {
    pub fn complete_started(&self, agent_id: String) {
        let _ = self
            .completion
            .try_send(StartAgentOutcome::Started { agent_id });
    }

    pub fn complete_error(&self, error: String) {
        let _ = self.completion.try_send(StartAgentOutcome::Error(error));
    }
}

pub struct StartAgentExecutor {
    terminal_view_id: EntityId,
}

impl StartAgentExecutor {
    pub fn new(terminal_view_id: EntityId) -> Self {
        Self { terminal_view_id }
    }

    /// Direct `StartAgent` actions (e.g. the BYOP `start_agent` tool) honor the
    /// profile's `run_agents` permission: only `AlwaysAllow` (or an autonomous app
    /// run) spawns without user approval; `AlwaysAsk` waits for the action card's
    /// Accept. The plan-driven orchestrator path is unaffected — `RunAgentsExecutor`
    /// does its own approval and dispatches into this executor directly.
    pub(super) fn should_autoexecute(&self, ctx: &ModelContext<Self>) -> bool {
        AppExecutionMode::as_ref(ctx).is_autonomous()
            || BlocklistAIPermissions::as_ref(ctx)
                .get_run_agents_setting(ctx, Some(self.terminal_view_id))
                .is_always_allow()
    }

    pub(super) fn preprocess_action(&mut self) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> impl Into<AnyActionExecution> + use<> {
        let AIAgentActionType::StartAgent {
            version,
            name,
            prompt,
            execution_mode,
            lifecycle_subscription,
        } = &input.action.action
        else {
            return ActionExecution::InvalidAction;
        };

        let version = *version;
        let receiver = self.dispatch(
            name.clone(),
            prompt.clone(),
            execution_mode.clone(),
            lifecycle_subscription.clone(),
            input.conversation_id,
            ctx,
        );

        ActionExecution::new_async(async move { receiver.recv().await }, move |result, _| {
            match result {
                Ok(StartAgentOutcome::Started { agent_id }) => {
                    AIAgentActionResultType::StartAgent(StartAgentResult::Success {
                        agent_id,
                        version,
                    })
                }
                Ok(StartAgentOutcome::Error(error)) => {
                    AIAgentActionResultType::StartAgent(StartAgentResult::Error { error, version })
                }
                Err(_) => {
                    AIAgentActionResultType::StartAgent(StartAgentResult::Cancelled { version })
                }
            }
        })
    }

    /// Dispatch a local child launch and return a receiver for its terminal-pane completion.
    pub fn dispatch(
        &mut self,
        name: String,
        prompt: String,
        execution_mode: StartAgentExecutionMode,
        lifecycle_subscription: Option<Vec<String>>,
        parent_conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) -> async_channel::Receiver<StartAgentOutcome> {
        let (sender, receiver) = async_channel::bounded(1);
        let (prompt, execution_mode) =
            normalize_legacy_local_child_harness_command(prompt, execution_mode);

        if let Err(error) = validate_execution_mode(&execution_mode) {
            let _ = sender.try_send(StartAgentOutcome::Error(error));
            return receiver;
        }

        ctx.emit(StartAgentExecutorEvent::CreateAgent(StartAgentRequest {
            name,
            prompt,
            execution_mode,
            lifecycle_subscription,
            parent_conversation_id,
            completion: sender,
        }));
        receiver
    }
}

fn validate_execution_mode(execution_mode: &StartAgentExecutionMode) -> Result<(), String> {
    match execution_mode {
        StartAgentExecutionMode::Local {
            harness_type: None, ..
        } => Ok(()),
        StartAgentExecutionMode::Local {
            harness_type: Some(harness_type),
            ..
        } => {
            let Some(harness) = Harness::parse_local_child_harness(harness_type) else {
                return Err(invalid_local_child_harness_error(harness_type));
            };
            if let Some(message) = local_harness_product_disabled_message(harness) {
                return Err(message.to_string());
            }
            Ok(())
        }
    }
}

fn invalid_local_child_harness_error(harness_type: &str) -> String {
    let harness_name = harness_type.trim();
    if harness_name.is_empty() {
        "Local child harness type is missing.".to_string()
    } else {
        format!("Unsupported local child harness '{harness_name}'.")
    }
}

fn parse_legacy_local_child_harness_command(command: &str) -> Option<(String, String)> {
    let args = split_shell_words(command.trim()).ok()?;
    match args.as_slice() {
        [binary, flag, child_prompt]
            if binary == "codex"
                && flag == "--dangerously-bypass-approvals-and-sandbox"
                && !child_prompt.trim().is_empty() =>
        {
            Some(("codex".to_string(), child_prompt.clone()))
        }
        _ => None,
    }
}

fn normalize_legacy_local_child_harness_command(
    prompt: String,
    execution_mode: StartAgentExecutionMode,
) -> (String, StartAgentExecutionMode) {
    match execution_mode {
        StartAgentExecutionMode::Local {
            harness_type: None,
            model_id,
        } => {
            if let Some((harness_type, child_prompt)) =
                parse_legacy_local_child_harness_command(&prompt)
            {
                (
                    child_prompt,
                    StartAgentExecutionMode::Local {
                        harness_type: Some(harness_type),
                        model_id,
                    },
                )
            } else {
                (
                    prompt,
                    StartAgentExecutionMode::Local {
                        harness_type: None,
                        model_id,
                    },
                )
            }
        }
        mode @ StartAgentExecutionMode::Local {
            harness_type: Some(_),
            ..
        } => (prompt, mode),
    }
}

impl Entity for StartAgentExecutor {
    type Event = StartAgentExecutorEvent;
}

pub enum StartAgentExecutorEvent {
    CreateAgent(StartAgentRequest),
}

#[cfg(test)]
#[path = "start_agent_tests.rs"]
mod tests;
