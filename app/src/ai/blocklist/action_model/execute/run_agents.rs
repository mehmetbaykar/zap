//! Local-only executor for `AIAgentActionType::RunAgents`.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use ai::agent::action::{RunAgentsAgentRunConfig, RunAgentsExecutionMode, RunAgentsRequest};
use ai::agent::action_result::{
    RunAgentsAgentOutcome, RunAgentsAgentOutcomeKind, RunAgentsLaunchedExecutionMode,
    RunAgentsResult,
};
use ai::agent::orchestration_config::{OrchestrationConfigStatus, OrchestrationExecutionMode};
use futures::FutureExt;
use futures::future::BoxFuture;
use warp_cli::agent::Harness;
use warp_core::execution_mode::AppExecutionMode;
use warpui::{Entity, EntityId, ModelContext, ModelHandle, SingletonEntity};

use super::start_agent::{StartAgentExecutor, StartAgentOutcome};
use super::{ActionExecution, AnyActionExecution, ExecuteActionInput};
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent::{
    AIAgentActionId, AIAgentActionResultType, AIAgentActionType, AIAgentInput,
    StartAgentExecutionMode,
};
use crate::ai::blocklist::{BlocklistAIHistoryModel, BlocklistAIPermissions};
use crate::ai::local_harness_setup::local_harness_product_disabled_message;

const SPAWN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy)]
pub struct RunAgentsSpawningSnapshot {
    pub agent_count: usize,
}

#[derive(Debug, Clone)]
struct ExistingLaunchedAgent {
    name: String,
    agent_id: String,
}

pub struct RunAgentsExecutor {
    pending: HashSet<AIAgentActionId>,
    launched_agents: HashMap<AIConversationId, HashMap<String, ExistingLaunchedAgent>>,
    start_agent_executor: ModelHandle<StartAgentExecutor>,
    terminal_view_id: EntityId,
}

pub enum RunAgentsExecutorEvent {
    SpawningStarted {
        action_id: AIAgentActionId,
        snapshot: RunAgentsSpawningSnapshot,
    },
    SpawningFinished {
        action_id: AIAgentActionId,
    },
}

impl Entity for RunAgentsExecutor {
    type Event = RunAgentsExecutorEvent;
}

impl RunAgentsExecutor {
    pub fn new(
        start_agent_executor: ModelHandle<StartAgentExecutor>,
        terminal_view_id: EntityId,
    ) -> Self {
        Self {
            pending: HashSet::new(),
            launched_agents: HashMap::new(),
            start_agent_executor,
            terminal_view_id,
        }
    }

    pub fn is_pending(&self, action_id: &AIAgentActionId) -> bool {
        self.pending.contains(action_id)
    }

    pub(super) fn cancel_execution(
        &mut self,
        action_id: &AIAgentActionId,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.pending.remove(action_id) {
            ctx.emit(RunAgentsExecutorEvent::SpawningFinished {
                action_id: action_id.clone(),
            });
        }
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> impl Into<AnyActionExecution> + use<> {
        let AIAgentActionType::RunAgents(request) = &input.action.action else {
            return ActionExecution::InvalidAction;
        };
        let mut request = request.clone();
        if let Some(reason) = prepare_request_for_execution(
            &mut request,
            input.conversation_id,
            self.terminal_view_id,
            &self.launched_agents,
            ctx,
        ) {
            return ActionExecution::Sync(AIAgentActionResultType::RunAgents(
                RunAgentsResult::Denied { reason },
            ));
        }

        let receiver = self.dispatch(input.action.id.clone(), request, input.conversation_id, ctx);
        ActionExecution::new_async(async move { receiver.recv().await }, |result, _| {
            AIAgentActionResultType::RunAgents(result.unwrap_or(RunAgentsResult::Cancelled))
        })
    }

    pub(super) fn should_autoexecute(
        &self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let AIAgentActionType::RunAgents(request) = &input.action.action else {
            return false;
        };
        if AppExecutionMode::as_ref(ctx).is_autonomous() {
            return true;
        }

        let mut resolved = request.clone();
        let plan_status =
            apply_approved_local_plan_config(&mut resolved, input.conversation_id, ctx);
        if plan_status.is_err()
            || plan_status
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .is_some_and(|status| status.is_approved())
            || duplicate_launched_agents_reason(
                &resolved,
                input.conversation_id,
                &self.launched_agents,
                ctx,
            )
            .is_some()
        {
            return true;
        }

        BlocklistAIPermissions::as_ref(ctx)
            .get_run_agents_setting(ctx, Some(self.terminal_view_id))
            .is_always_allow()
    }

    pub(super) fn preprocess_action(&mut self) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }

    fn dispatch(
        &mut self,
        action_id: AIAgentActionId,
        request: RunAgentsRequest,
        parent_conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) -> async_channel::Receiver<RunAgentsResult> {
        let (sender, receiver) = async_channel::bounded(1);
        if self.pending.contains(&action_id) {
            let _ = sender.try_send(RunAgentsResult::Cancelled);
            return receiver;
        }
        if let Err(error) = validate_request(&request) {
            let _ = sender.try_send(RunAgentsResult::Failure { error });
            return receiver;
        }

        self.pending.insert(action_id.clone());
        ctx.emit(RunAgentsExecutorEvent::SpawningStarted {
            action_id: action_id.clone(),
            snapshot: RunAgentsSpawningSnapshot {
                agent_count: request.agent_run_configs.len(),
            },
        });

        let mut slots = Vec::with_capacity(request.agent_run_configs.len());
        for config in &request.agent_run_configs {
            let prompt = compose_run_agents_child_prompt(&request.base_prompt, &config.prompt);
            let execution_mode = match run_agents_to_start_agent_mode(
                &request.execution_mode,
                &request.harness_type,
                &request.model_id,
            ) {
                Ok(mode) => mode,
                Err(error) => {
                    slots.push(ChildSlot::Failed(error));
                    continue;
                }
            };
            let receiver = self.start_agent_executor.update(ctx, |executor, ctx| {
                executor.dispatch(
                    config.name.clone(),
                    prompt,
                    execution_mode,
                    None,
                    parent_conversation_id,
                    ctx,
                )
            });
            slots.push(ChildSlot::Pending(receiver));
        }

        let configs = request.agent_run_configs.clone();
        let model_id = request.model_id.clone();
        let harness_type = request.harness_type.clone();
        ctx.spawn(
            async move {
                let mut outcomes = Vec::with_capacity(slots.len());
                for slot in slots {
                    let outcome = match slot {
                        ChildSlot::Failed(error) => RunAgentsAgentOutcomeKind::Failed { error },
                        ChildSlot::Pending(receiver) => {
                            let timeout = warpui::r#async::Timer::after(SPAWN_TIMEOUT);
                            match futures::future::select(
                                Box::pin(receiver.recv()),
                                Box::pin(timeout),
                            )
                            .await
                            {
                                futures::future::Either::Left((
                                    Ok(StartAgentOutcome::Started { agent_id }),
                                    _,
                                )) => RunAgentsAgentOutcomeKind::Launched { agent_id },
                                futures::future::Either::Left((
                                    Ok(StartAgentOutcome::Error(error)),
                                    _,
                                )) => RunAgentsAgentOutcomeKind::Failed { error },
                                futures::future::Either::Left((Err(_), _)) => {
                                    RunAgentsAgentOutcomeKind::Failed {
                                        error: "Cancelled before launch".to_string(),
                                    }
                                }
                                futures::future::Either::Right((_, _)) => {
                                    RunAgentsAgentOutcomeKind::Failed {
                                        error: format!(
                                            "Agent failed to start within {} seconds. The local harness binary may not be installed.",
                                            SPAWN_TIMEOUT.as_secs()
                                        ),
                                    }
                                }
                            }
                        }
                    };
                    outcomes.push(outcome);
                }
                outcomes
            },
            move |me, outcomes, ctx| {
                if !me.pending.remove(&action_id) {
                    return;
                }
                let agents = build_agent_outcomes(&configs, outcomes, &model_id);
                me.record_launched_agents(parent_conversation_id, &agents);
                ctx.emit(RunAgentsExecutorEvent::SpawningFinished {
                    action_id: action_id.clone(),
                });
                let _ = sender.try_send(RunAgentsResult::Launched {
                    model_id,
                    harness_type,
                    execution_mode: RunAgentsLaunchedExecutionMode::Local,
                    agents,
                });
            },
        );

        receiver
    }

    fn record_launched_agents(
        &mut self,
        conversation_id: AIConversationId,
        agents: &[RunAgentsAgentOutcome],
    ) {
        for agent in agents {
            let RunAgentsAgentOutcomeKind::Launched { agent_id } = &agent.kind else {
                continue;
            };
            let Some(normalized_name) = normalize_agent_name(&agent.name) else {
                continue;
            };
            self.launched_agents
                .entry(conversation_id)
                .or_default()
                .insert(
                    normalized_name,
                    ExistingLaunchedAgent {
                        name: agent.name.clone(),
                        agent_id: agent_id.clone(),
                    },
                );
        }
    }
}

enum ChildSlot {
    Failed(String),
    Pending(async_channel::Receiver<StartAgentOutcome>),
}

fn prepare_request_for_execution(
    request: &mut RunAgentsRequest,
    parent_conversation_id: AIConversationId,
    terminal_view_id: EntityId,
    launched_agents: &HashMap<AIConversationId, HashMap<String, ExistingLaunchedAgent>>,
    ctx: &ModelContext<RunAgentsExecutor>,
) -> Option<String> {
    let status = match apply_approved_local_plan_config(request, parent_conversation_id, ctx) {
        Ok(status) => status,
        Err(reason) => return Some(reason),
    };
    if let Some(reason) =
        duplicate_launched_agents_reason(request, parent_conversation_id, launched_agents, ctx)
    {
        return Some(reason);
    }
    if AppExecutionMode::as_ref(ctx).is_autonomous() {
        return None;
    }
    if status.is_some_and(|status| status.is_disapproved()) {
        return Some("Orchestration config was disapproved".to_string());
    }
    if BlocklistAIPermissions::as_ref(ctx)
        .get_run_agents_setting(ctx, Some(terminal_view_id))
        .is_never_allow()
    {
        return Some(
            "Running child agents is disabled by the active execution profile.".to_string(),
        );
    }
    None
}

fn apply_approved_local_plan_config(
    request: &mut RunAgentsRequest,
    parent_conversation_id: AIConversationId,
    ctx: &ModelContext<RunAgentsExecutor>,
) -> Result<Option<OrchestrationConfigStatus>, String> {
    let Some((config, status)) = BlocklistAIHistoryModel::as_ref(ctx)
        .conversation(&parent_conversation_id)
        .and_then(|conversation| conversation.orchestration_config_for_plan(&request.plan_id))
    else {
        return Ok(None);
    };

    if status.is_approved() {
        match config.execution_mode {
            OrchestrationExecutionMode::Local => {
                request.model_id = config.model_id.clone();
                request.harness_type = config.harness_type.clone();
            }
            OrchestrationExecutionMode::Remote { .. } => {
                return Err("Cloud child-agent execution is not available in Zap.".to_string());
            }
        }
    }
    Ok(Some(status))
}

fn duplicate_launched_agents_reason(
    request: &RunAgentsRequest,
    parent_conversation_id: AIConversationId,
    launched_agents: &HashMap<AIConversationId, HashMap<String, ExistingLaunchedAgent>>,
    ctx: &ModelContext<RunAgentsExecutor>,
) -> Option<String> {
    let requested = request
        .agent_run_configs
        .iter()
        .map(|config| normalize_agent_name(&config.name))
        .collect::<Option<Vec<_>>>()?;
    if requested.is_empty() {
        return None;
    }

    let existing =
        existing_launched_agents_for_conversation(parent_conversation_id, launched_agents, ctx);
    let duplicates = requested
        .iter()
        .map(|name| existing.get(name))
        .collect::<Option<Vec<_>>>()?;
    let duplicate_list = duplicates
        .iter()
        .map(|agent| format!("{} ({})", agent.name, agent.agent_id))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "Requested agent(s) have already been launched: {duplicate_list}. Duplicate launch rejected."
    ))
}

fn existing_launched_agents_for_conversation(
    parent_conversation_id: AIConversationId,
    launched_agents: &HashMap<AIConversationId, HashMap<String, ExistingLaunchedAgent>>,
    ctx: &ModelContext<RunAgentsExecutor>,
) -> HashMap<String, ExistingLaunchedAgent> {
    let mut existing = launched_agents
        .get(&parent_conversation_id)
        .cloned()
        .unwrap_or_default();
    if let Some(conversation) =
        BlocklistAIHistoryModel::as_ref(ctx).conversation(&parent_conversation_id)
    {
        for exchange in conversation.all_exchanges() {
            for input in &exchange.input {
                let AIAgentInput::ActionResult { result, .. } = input else {
                    continue;
                };
                let AIAgentActionResultType::RunAgents(RunAgentsResult::Launched {
                    agents, ..
                }) = &result.result
                else {
                    continue;
                };
                for agent in agents {
                    let RunAgentsAgentOutcomeKind::Launched { agent_id } = &agent.kind else {
                        continue;
                    };
                    let Some(name) = normalize_agent_name(&agent.name) else {
                        continue;
                    };
                    existing
                        .entry(name)
                        .or_insert_with(|| ExistingLaunchedAgent {
                            name: agent.name.clone(),
                            agent_id: agent_id.clone(),
                        });
                }
            }
        }
    }
    existing
}

fn normalize_agent_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_ascii_lowercase())
}

fn validate_request(request: &RunAgentsRequest) -> Result<(), String> {
    if request.agent_run_configs.is_empty() {
        return Err("orchestrate: empty agent_run_configs".to_string());
    }
    run_agents_to_start_agent_mode(
        &request.execution_mode,
        &request.harness_type,
        &request.model_id,
    )?;
    Ok(())
}

pub fn compose_run_agents_child_prompt(base_prompt: &str, per_agent_prompt: &str) -> String {
    match (
        base_prompt.trim().is_empty(),
        per_agent_prompt.trim().is_empty(),
    ) {
        (false, false) => format!("{base_prompt}\n\n{per_agent_prompt}"),
        (false, true) => base_prompt.to_string(),
        (true, false) => per_agent_prompt.to_string(),
        (true, true) => String::new(),
    }
}

pub fn run_agents_to_start_agent_mode(
    execution_mode: &RunAgentsExecutionMode,
    harness_type: &str,
    model_id: &str,
) -> Result<StartAgentExecutionMode, String> {
    match execution_mode {
        RunAgentsExecutionMode::Local => {
            let model_id = (!model_id.trim().is_empty()).then(|| model_id.trim().to_string());
            let harness_type = harness_type.trim();
            if harness_type.is_empty() || harness_type.eq_ignore_ascii_case("oz") {
                return Ok(StartAgentExecutionMode::Local {
                    harness_type: None,
                    model_id,
                });
            }

            let Some(harness) = Harness::parse_local_child_harness(harness_type) else {
                return Err(format!("Unsupported local child harness '{harness_type}'."));
            };
            if let Some(message) = local_harness_product_disabled_message(harness) {
                return Err(message.to_string());
            }
            Ok(StartAgentExecutionMode::Local {
                harness_type: Some(harness.to_string()),
                model_id,
            })
        }
    }
}

fn build_agent_outcomes(
    configs: &[RunAgentsAgentRunConfig],
    outcomes: Vec<RunAgentsAgentOutcomeKind>,
    batch_model_id: &str,
) -> Vec<RunAgentsAgentOutcome> {
    configs
        .iter()
        .zip(outcomes)
        .map(|(config, kind)| RunAgentsAgentOutcome {
            name: config.name.clone(),
            kind,
            // Report what this child actually ran on: its own override when it
            // set one, otherwise the batch model it inherited.
            resolved_model_id: if config.model_id.trim().is_empty() {
                batch_model_id.to_string()
            } else {
                config.model_id.clone()
            },
        })
        .collect()
}

#[cfg(test)]
#[path = "run_agents_tests.rs"]
mod tests;
