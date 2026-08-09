//! Implementation of terminal panes.
#[cfg(not(target_family = "wasm"))]
use std::collections::HashMap;
use std::sync::mpsc::SyncSender;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use warp_cli::agent::Harness;
use warp_core::execution_mode::AppExecutionMode;
use warp_multi_agent_api as multi_agent_api;
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warpui::{
    AppContext, EntityId, ModelHandle, SingletonEntity, ViewContext, ViewHandle, WindowId,
};

#[cfg(not(target_family = "wasm"))]
use super::local_harness_launch::{PreparedLocalHarnessLaunch, prepare_local_harness_child_launch};
use super::{
    DetachType, PaneConfiguration, PaneContent, PaneId, PaneStackEvent, PaneView, ShareableLink,
    ShareableLinkError, TerminalPaneId,
};
use crate::AIExecutionProfilesModel;
use crate::ai::agent::StartAgentExecutionMode;
use crate::ai::agent::conversation::{AIConversationId, ConversationStatus};
use crate::ai::ambient_agents::task::normalize_orchestrator_agent_name;
#[cfg(feature = "local_fs")]
use crate::ai::blocklist::BlocklistAIHistoryEvent;
use crate::ai::blocklist::agent_view::AgentViewEntryOrigin;
use crate::ai::blocklist::{BlocklistAIHistoryModel, StartAgentRequest};
use crate::ai::conversation_utils;
use crate::ai::llms::LLMPreferences;
use crate::ai::skills::SkillManager;
use crate::app_state::{AmbientAgentPaneSnapshot, LeafContents, TerminalPaneSnapshot};
#[cfg(feature = "local_fs")]
use crate::pane_group::CodeSource;
use crate::pane_group::Event::OpenConversationHistory;
use crate::pane_group::child_agent::{
    ErrorChildAgentConversationRequest, create_error_child_agent_conversation,
};
#[cfg(not(target_family = "wasm"))]
use crate::pane_group::child_agent::{
    HiddenChildAgentConversation, HiddenChildAgentConversationRequest, HiddenChildAgentTaskContext,
    create_hidden_child_agent_conversation,
};
use crate::pane_group::{self, Direction, PaneGroup};
use crate::persistence::{BlockCompleted, ModelEvent};
use crate::session_management::SessionNavigationData;
use crate::terminal::cli_agent_sessions::CLIAgentSessionsModel;
use crate::terminal::general_settings::GeneralSettings;
use crate::terminal::shared_session::SharedSessionStatus;
use crate::terminal::view::Event;
use crate::terminal::{TerminalManager, TerminalView};
use crate::view_components::ToastFlavor;
use crate::workspace::sync_inputs::SyncedInputState;
use crate::workspace::{PaneViewLocator, WorkspaceRegistry};

pub type TerminalPaneView = PaneView<TerminalView>;

/// Data kept for terminal panes.
pub struct TerminalPane {
    model_event_sender: Option<SyncSender<ModelEvent>>,

    /// Used to uniquely identify the pane, even across separate runs of the app.
    uuid: Vec<u8>,

    pane_configuration: ModelHandle<PaneConfiguration>,

    /// Defining `terminal_manager` before `view` means that `terminal_manager`
    /// gets dropped first (guaranteed by the language), which halts the event
    /// loop and avoids possible deadlocks during session cleanup. This is enforced
    /// by the `PaneStack`, since the terminal manager is the associated data for
    /// the backing pane view.
    view: ViewHandle<TerminalPaneView>,
}

fn resolve_runtime_skills(
    skill_references: &[ai::skills::SkillReference],
    ctx: &AppContext,
) -> Result<Vec<String>, Vec<String>> {
    let skill_manager = SkillManager::as_ref(ctx);
    let mut runtime_skills = Vec::with_capacity(skill_references.len());
    let mut unresolved_references = Vec::new();

    for reference in skill_references {
        let Some(skill) = skill_manager.skill_by_reference(reference) else {
            unresolved_references.push(reference.to_string());
            continue;
        };
        runtime_skills.push(serialize_proto_to_base64(&multi_agent_api::Skill::from(
            skill.clone(),
        )));
    }

    if unresolved_references.is_empty() {
        Ok(runtime_skills)
    } else {
        Err(unresolved_references)
    }
}

fn serialize_proto_to_base64<M: prost::Message>(message: &M) -> String {
    BASE64_STANDARD.encode(message.encode_to_vec())
}

impl TerminalPane {
    pub(in crate::pane_group) fn new(
        uuid: Vec<u8>,
        terminal_manager: ModelHandle<Box<dyn TerminalManager>>,
        terminal_view: ViewHandle<TerminalView>,
        model_event_sender: Option<SyncSender<ModelEvent>>,
        ctx: &mut ViewContext<PaneGroup>,
    ) -> Self {
        let pane_configuration = terminal_view.as_ref(ctx).pane_configuration().to_owned();
        let view = ctx.add_typed_action_view(|ctx| {
            let pane_id = PaneId::from_terminal_pane_ctx(ctx);
            PaneView::new(
                pane_id,
                terminal_view,
                terminal_manager,
                pane_configuration.clone(),
                ctx,
            )
        });

        Self {
            model_event_sender,
            uuid,
            pane_configuration,
            view,
        }
    }

    /// The [`PaneView<TerminalView>`] for this pane.
    #[cfg(any(test, feature = "integration_tests"))]
    pub(in crate::pane_group) fn pane_view(&self) -> ViewHandle<TerminalPaneView> {
        self.view.to_owned()
    }

    /// The [`TerminalView`] backing the [`PaneView`] for this terminal pane.
    pub(crate) fn terminal_view(&self, ctx: &AppContext) -> ViewHandle<TerminalView> {
        self.view.as_ref(ctx).child(ctx)
    }

    /// The UUID that identifies this terminal session across app restarts.
    pub(in crate::pane_group) fn session_uuid(&self) -> Vec<u8> {
        self.uuid.clone()
    }

    /// The terminal manager responsible for this session's event loop.
    pub(in crate::pane_group) fn terminal_manager(
        &self,
        ctx: &AppContext,
    ) -> ModelHandle<Box<dyn TerminalManager>> {
        self.view.as_ref(ctx).child_data(ctx).clone()
    }

    /// Instructs the SQLite thread to delete blocks for this session.
    pub(in crate::pane_group) fn delete_blocks(&self, ctx: &AppContext) {
        if !AppExecutionMode::as_ref(ctx).can_save_session() {
            return;
        }

        if let Some(sender) = &self.model_event_sender {
            let model_event = ModelEvent::DeleteBlocks(self.uuid.clone());
            if let Err(err) = sender.send(model_event) {
                log::error!(
                    "Error sending blocks deleted event for terminal id {} {:?}",
                    self.terminal_view(ctx).id(),
                    err
                );
            }
        }
    }

    pub fn session_navigation_data(
        &self,
        pane_group_id: EntityId,
        window_id: WindowId,
        app: &AppContext,
    ) -> SessionNavigationData {
        let view = self.terminal_view(app).as_ref(app);
        SessionNavigationData::new(
            view.full_prompt(app),
            view.prompt_elements(app),
            view.session_command_context(app),
            PaneViewLocator {
                pane_group_id,
                pane_id: self.id(),
            },
            view.last_focus_ts(),
            view.is_read_only(),
            window_id,
            view.model.lock().shared_session_status().clone(),
        )
    }

    pub fn terminal_pane_id(&self) -> TerminalPaneId {
        self.id()
            .as_terminal_pane_id()
            .expect("Should be able to derive a TerminalPaneId from TerminalPane")
    }
}

impl PaneContent for TerminalPane {
    fn id(&self) -> PaneId {
        PaneId::from_terminal_pane_view(&self.view)
    }

    fn attach(
        &self,
        group: &PaneGroup,
        focus_handle: crate::pane_group::focus_state::PaneFocusHandle,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        // TODO(ben): As much as possible, logic from PaneGroup::add_session should go here.
        //  This will simplify PaneGroup, especially when implementing pane management.
        let terminal_pane_id = self.terminal_pane_id();

        self.view
            .update(ctx, |view, ctx| view.set_focus_handle(focus_handle, ctx));

        // Attach the initial terminal view in the stack.
        attach_terminal_view(&self.terminal_view(ctx), terminal_pane_id, ctx);

        // Subscribe to the pane stack to handle views being pushed/popped.
        let pane_stack = self.view.as_ref(ctx).pane_stack().clone();
        ctx.subscribe_to_model(&pane_stack, move |group, _, event, ctx| {
            handle_pane_stack_event(group, event, terminal_pane_id, ctx);
        });

        ctx.subscribe_to_view(&self.view, move |group, _, event, ctx| {
            group.handle_pane_view_event(terminal_pane_id.into(), event, ctx);
        });

        if SyncedInputState::as_ref(ctx).should_sync_this_pane_group(ctx.view_id(), ctx.window_id())
            && let Some(active_pane_view) = group.active_session_view(ctx)
        {
            let event = active_pane_view
                .as_ref(ctx)
                .create_sync_event_based_on_terminal_state(ctx);

            group.send_sync_event_to_session(terminal_pane_id, &event, ctx);
        }

        let terminal_view_id = self.terminal_view(ctx).id();

        #[cfg(feature = "local_fs")]
        {
            ctx.subscribe_to_model(
                &BlocklistAIHistoryModel::handle(ctx),
                move |group, _, event, ctx| {
                    let Some(model_event_sender) = group.model_event_sender.clone() else {
                        return;
                    };

                    let is_shared_ambient_agent_session = group
                        .terminal_view_from_pane_id(terminal_pane_id, ctx)
                        .map(|view| {
                            view.as_ref(ctx)
                                .model
                                .lock()
                                .is_shared_ambient_agent_session()
                        })
                        .unwrap_or(false);

                    handle_ai_history_event(
                        event,
                        terminal_view_id,
                        terminal_pane_id,
                        model_event_sender,
                        is_shared_ambient_agent_session,
                        ctx,
                    );
                },
            );
        }

        // Store the pane group entity ID on the agent view controller so the
        // message bar can perform pane-group-scoped visibility checks.
        let pane_group_id = ctx.view_id();
        let terminal_view = self.terminal_view(ctx);
        let agent_view_controller = terminal_view.as_ref(ctx).agent_view_controller().clone();
        agent_view_controller.update(ctx, |controller, _ctx| {
            controller.set_pane_group_id(pane_group_id);
        });
        let _ = (agent_view_controller, terminal_view_id);
    }

    fn detach(
        &self,
        _group: &PaneGroup,
        detach_type: DetachType,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        if matches!(detach_type, DetachType::Closed) {
            // Only immediately clear conversations and delete blocks if the session is being
            // permanently closed.
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history_model, ctx| {
                history_model
                    .clear_conversations_for_terminal_surface(self.terminal_view(ctx).id(), ctx);
            });
            self.delete_blocks(ctx);
        }

        // Unsubscribe from all views in the pane stack.
        let pane_stack = self.view.as_ref(ctx).pane_stack().clone();
        let contents = pane_stack.as_ref(ctx).entries().to_vec();
        for (manager, view) in contents {
            // Notify the view that it's being detached so it can react appropriately
            // (e.g. the shared-session viewer tears down its network only when the detach
            // is not reversible).
            manager.update(ctx, |terminal_manager, ctx| {
                terminal_manager.on_view_detached(detach_type, ctx);
            });
            ctx.unsubscribe_to_view(&view);
        }

        let terminal_view_id = self.terminal_view(ctx).id();

        // Clean up any active CLI agent session so its notification is removed.
        // Skip this for moves — the session is still running and will re-register in the new tab.
        if !matches!(detach_type, DetachType::Moved) {
            CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
                sessions.remove_session(terminal_view_id, ctx);
            });
        }

        ctx.unsubscribe_to_model(&pane_stack);

        ctx.unsubscribe_to_view(&self.view);

        #[cfg(feature = "local_fs")]
        {
            ctx.unsubscribe_to_model(&BlocklistAIHistoryModel::handle(ctx));
        }
    }

    fn snapshot(&self, app: &AppContext) -> LeafContents {
        let view = self.terminal_view(app).as_ref(app);
        let is_active = view.is_active_session(app);

        // Capture the current input_config from the AI input model
        let current_input_config = view.input_config(app.as_ref());

        if view.model.lock().shared_session_status().is_viewer() {
            // We save and restore ambient agent sessions
            // (restoring the shared session if it's still open and the conversation transcript otherwise).
            if let Some(ambient_model) = view.ambient_agent_view_model() {
                let ambient_model = ambient_model.as_ref(app);
                if ambient_model.is_ambient_agent() {
                    let task_id = ambient_model.task_id();

                    return LeafContents::AmbientAgent(AmbientAgentPaneSnapshot {
                        uuid: self.uuid.clone(),
                        task_id,
                    });
                }
            }

            LeafContents::Terminal(TerminalPaneSnapshot {
                uuid: self.uuid.clone(),
                cwd: None,
                is_active,
                is_read_only: false,
                shell_launch_data: None,
                input_config: None,
                llm_model_override: None,
                active_profile_id: None,
                conversation_ids_to_restore: vec![],
                active_conversation_id: None,
            })
        } else if view.model.lock().is_conversation_transcript_viewer() {
            // Conversation transcript viewers (opened from the conversation list)
            // can be restored via the ambient agent task if one exists.
            let task_id = view.model.lock().ambient_agent_task_id();
            if task_id.is_some() {
                LeafContents::AmbientAgent(AmbientAgentPaneSnapshot {
                    uuid: self.uuid.clone(),
                    task_id,
                })
            } else {
                LeafContents::Terminal(TerminalPaneSnapshot {
                    uuid: self.uuid.clone(),
                    cwd: None,
                    is_active,
                    is_read_only: false,
                    shell_launch_data: None,
                    input_config: None,
                    llm_model_override: None,
                    active_profile_id: None,
                    conversation_ids_to_restore: vec![],
                    active_conversation_id: None,
                })
            }
        } else {
            let llm_model_override =
                LLMPreferences::as_ref(app).get_base_llm_override(self.terminal_view(app).id());

            let active_profile_id = AIExecutionProfilesModel::as_ref(app)
                .active_profile(Some(self.terminal_view(app).id()), app)
                .sync_id();

            // Collect all conversation IDs for this terminal view
            let conversation_ids_to_restore = BlocklistAIHistoryModel::as_ref(app)
                .all_live_conversations_for_terminal_surface(self.terminal_view(app).id())
                .map(|conversation| conversation.id())
                .collect();

            // Capture agent view state: if fullscreen, store the active conversation ID
            let active_conversation_id = view
                .agent_view_controller()
                .as_ref(app)
                .agent_view_state()
                .display_mode()
                .filter(|mode| mode.is_fullscreen())
                .and_then(|_| {
                    view.agent_view_controller()
                        .as_ref(app)
                        .agent_view_state()
                        .active_conversation_id()
                });

            LeafContents::Terminal(TerminalPaneSnapshot {
                uuid: self.uuid.clone(),
                cwd: view.pwd_if_local(app),
                is_active,
                is_read_only: view.model.lock().is_read_only(),
                shell_launch_data: view.shell_launch_data_if_local(app),
                input_config: Some(current_input_config),
                llm_model_override,
                active_profile_id,
                conversation_ids_to_restore,
                active_conversation_id,
            })
        }
    }

    fn has_application_focus(&self, ctx: &mut ViewContext<PaneGroup>) -> bool {
        self.view.is_self_or_child_focused(ctx)
    }

    fn focus(&self, ctx: &mut ViewContext<PaneGroup>) {
        self.terminal_view(ctx)
            .update(ctx, |view, ctx| view.redetermine_global_focus(ctx));
    }

    fn shareable_link(
        &self,
        ctx: &mut ViewContext<PaneGroup>,
    ) -> Result<ShareableLink, ShareableLinkError> {
        let manager = self.terminal_manager(ctx);
        let the_model = manager.as_ref(ctx).model();
        let lock = the_model.lock();

        // Check if this is a conversation transcript viewer
        if lock.is_conversation_transcript_viewer() {
            // Zap does not expose Warp-hosted conversation transcript URLs.
            return Err(ShareableLinkError::Expected);
        }

        // Check for shared session status
        let session_status = lock.shared_session_status();
        match session_status {
            SharedSessionStatus::NotShared => Ok(ShareableLink::Base),
            SharedSessionStatus::ActiveViewer { role: _ } => Err(ShareableLinkError::Expected),
            _ => Err(ShareableLinkError::Expected),
        }
    }

    fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    fn is_pane_being_dragged(&self, ctx: &AppContext) -> bool {
        self.view.as_ref(ctx).is_being_dragged()
    }
}

#[derive(Clone, Copy)]
struct AgentConversationActionState {
    owner_terminal_view_id: EntityId,
    is_in_progress: bool,
}

fn agent_conversation_action_state(
    conversation_id: AIConversationId,
    ctx: &AppContext,
) -> Option<AgentConversationActionState> {
    let history_model = BlocklistAIHistoryModel::as_ref(ctx);
    let conversation = history_model.conversation(&conversation_id)?;
    let owner_terminal_view_id =
        history_model.terminal_surface_id_for_conversation(&conversation_id)?;
    Some(AgentConversationActionState {
        owner_terminal_view_id,
        is_in_progress: conversation.status().is_in_progress(),
    })
}

fn terminal_view_for_owner_in_group(
    group: &PaneGroup,
    owner_terminal_view_id: EntityId,
    ctx: &AppContext,
) -> Option<ViewHandle<TerminalView>> {
    let pane_id = group.find_pane_id_for_terminal_view(owner_terminal_view_id, ctx)?;
    group.terminal_view_from_pane_id(pane_id, ctx)
}

fn pane_group_and_terminal_view_for_owner(
    owner_terminal_view_id: EntityId,
    ctx: &AppContext,
) -> Option<(ViewHandle<PaneGroup>, ViewHandle<TerminalView>)> {
    WorkspaceRegistry::as_ref(ctx)
        .all_workspaces(ctx)
        .into_iter()
        .find_map(|(_, workspace)| {
            workspace.as_ref(ctx).tab_views().find_map(|pane_group| {
                terminal_view_for_owner_in_group(
                    pane_group.as_ref(ctx),
                    owner_terminal_view_id,
                    ctx,
                )
                .map(|terminal_view| (pane_group.clone(), terminal_view))
            })
        })
}

fn stop_local_agent_conversation(
    group: &PaneGroup,
    owner_terminal_view_id: EntityId,
    conversation_id: AIConversationId,
    ctx: &mut ViewContext<PaneGroup>,
) -> bool {
    let terminal_view = terminal_view_for_owner_in_group(group, owner_terminal_view_id, ctx)
        .or_else(|| {
            pane_group_and_terminal_view_for_owner(owner_terminal_view_id, ctx)
                .map(|(_, terminal_view)| terminal_view)
        });
    let Some(terminal_view) = terminal_view else {
        log::warn!(
            "StopAgentConversation: no terminal view found for conversation {conversation_id:?}"
        );
        return false;
    };

    terminal_view.update(ctx, |terminal_view, ctx| {
        terminal_view.stop_local_agent_conversation(conversation_id, ctx);
    });
    true
}

fn stop_agent_conversation(
    group: &PaneGroup,
    conversation_id: AIConversationId,
    ctx: &mut ViewContext<PaneGroup>,
) {
    let Some(state) = agent_conversation_action_state(conversation_id, ctx) else {
        log::warn!("StopAgentConversation: conversation {conversation_id:?} not found");
        return;
    };
    if !state.is_in_progress {
        return;
    }
    if !stop_local_agent_conversation(group, state.owner_terminal_view_id, conversation_id, ctx) {
        BlocklistAIHistoryModel::handle(ctx).update(ctx, |history_model, ctx| {
            history_model.update_conversation_status(
                state.owner_terminal_view_id,
                conversation_id,
                ConversationStatus::Cancelled,
                ctx,
            );
        });
    }
}

fn pane_group_hosting_split_off_child(
    conversation_id: AIConversationId,
    ctx: &AppContext,
) -> Option<ViewHandle<PaneGroup>> {
    WorkspaceRegistry::as_ref(ctx)
        .all_workspaces(ctx)
        .into_iter()
        .find_map(|(_, workspace)| {
            workspace.as_ref(ctx).tab_views().find_map(|pane_group| {
                pane_group
                    .as_ref(ctx)
                    .child_agent_origin()
                    .is_some_and(|origin| origin.conversation_id == conversation_id)
                    .then(|| pane_group.clone())
            })
        })
}

fn discard_child_agent_pane_in_group(
    group: &mut PaneGroup,
    conversation_id: AIConversationId,
    ctx: &mut ViewContext<PaneGroup>,
) -> bool {
    let tracked_child_pane = group.child_agent_panes.remove(&conversation_id);
    let owner_child_pane = BlocklistAIHistoryModel::as_ref(ctx)
        .terminal_surface_id_for_conversation(&conversation_id)
        .and_then(|terminal_view_id| group.find_pane_id_for_terminal_view(terminal_view_id, ctx));
    let Some(child_pane_id) = tracked_child_pane.or(owner_child_pane) else {
        return false;
    };

    if group
        .child_agent_origin
        .as_ref()
        .is_some_and(|origin| origin.conversation_id == conversation_id)
    {
        group.child_agent_origin = None;
    }

    let was_focused = group.focus_state.as_ref(ctx).is_pane_focused(child_pane_id);
    if let Some(terminal_view) = group.terminal_view_from_pane_id(child_pane_id, ctx) {
        terminal_view.update(ctx, |view, ctx| {
            view.clear_orchestration_split_off(ctx);
            view.shutdown_pty(ctx);
        });
    }

    if let Some(original_pane_id) = group.panes.original_pane_for_replacement(child_pane_id) {
        group.panes.revert_temporary_replacement(child_pane_id);
        if was_focused {
            group.focus_pane(original_pane_id, true, ctx);
        }
    } else {
        group.panes.remove_hidden_pane(child_pane_id);
    }

    let is_in_tree = group.panes.is_pane_in_tree(child_pane_id);
    if is_in_tree && group.panes.visible_pane_count() <= 1 {
        ctx.emit(pane_group::Event::Exited {
            add_to_undo_stack: false,
        });
        return true;
    }

    if is_in_tree {
        group.focus_next_terminal_pane_and_activate_session(
            child_pane_id,
            pane_group::PaneRemovalReason::Close,
            ctx,
        );
    }

    let discarded = group.cleanup_closed_pane(child_pane_id, ctx);
    group.handle_pane_count_change(ctx);
    discarded
}

fn discard_child_agent_pane_for_conversation(
    group: &mut PaneGroup,
    owner_terminal_view_id: Option<EntityId>,
    conversation_id: AIConversationId,
    ctx: &mut ViewContext<PaneGroup>,
) -> bool {
    if discard_child_agent_pane_in_group(group, conversation_id, ctx) {
        return true;
    }
    if let Some(split_off_pane_group) = pane_group_hosting_split_off_child(conversation_id, ctx)
        && split_off_pane_group.id() != ctx.view_id()
        && split_off_pane_group.update(ctx, |pane_group, ctx| {
            discard_child_agent_pane_in_group(pane_group, conversation_id, ctx)
        })
    {
        return true;
    }

    let Some(owner_terminal_view_id) = owner_terminal_view_id else {
        return false;
    };
    let Some((owner_pane_group, _)) =
        pane_group_and_terminal_view_for_owner(owner_terminal_view_id, ctx)
    else {
        return false;
    };
    if owner_pane_group.id() == ctx.view_id() {
        return false;
    }

    owner_pane_group.update(ctx, |pane_group, ctx| {
        discard_child_agent_pane_in_group(pane_group, conversation_id, ctx)
    })
}

fn kill_agent_conversation(
    group: &mut PaneGroup,
    source_terminal_view_id: Option<EntityId>,
    conversation_id: AIConversationId,
    ctx: &mut ViewContext<PaneGroup>,
) {
    let state = agent_conversation_action_state(conversation_id, ctx);

    if let Some(state) = state
        && state.is_in_progress
    {
        stop_local_agent_conversation(group, state.owner_terminal_view_id, conversation_id, ctx);
    }

    let owner_terminal_view_id = state
        .map(|state| state.owner_terminal_view_id)
        .or(source_terminal_view_id);
    if !discard_child_agent_pane_for_conversation(
        group,
        owner_terminal_view_id,
        conversation_id,
        ctx,
    ) {
        log::warn!("KillAgentConversation: no child pane found for {conversation_id:?}");
    }

    if owner_terminal_view_id.is_none() {
        log::warn!(
            "KillAgentConversation: no terminal view found for conversation {conversation_id:?}"
        );
    }
    conversation_utils::delete_conversation(conversation_id, owner_terminal_view_id, ctx);
}

/// Attaches a terminal view to the pane group by subscribing to its events
/// and setting the file tree code model.
fn attach_terminal_view(
    terminal_view: &ViewHandle<TerminalView>,
    terminal_pane_id: TerminalPaneId,
    ctx: &mut ViewContext<PaneGroup>,
) {
    ctx.subscribe_to_view(
        terminal_view,
        move |group: &mut PaneGroup, _, event, ctx| {
            handle_terminal_view_event(group, terminal_pane_id, event, ctx);
        },
    );
}

/// Handles events from the pane stack when views are added or removed.
fn handle_pane_stack_event(
    group: &mut PaneGroup,
    event: &PaneStackEvent<TerminalView>,
    terminal_pane_id: TerminalPaneId,
    ctx: &mut ViewContext<PaneGroup>,
) {
    match event {
        PaneStackEvent::ViewAdded(terminal_view) => {
            attach_terminal_view(terminal_view, terminal_pane_id, ctx);
        }
        PaneStackEvent::ViewRemoved(terminal_view) => {
            ctx.unsubscribe_to_view(terminal_view);
        }
    }

    // Ensure we use the new top-level view's title and active session status.
    // TODO(ben): This shouldn't be necessary once titles are set declaratively.
    if let Some(active_terminal) = group.terminal_view_from_pane_id(terminal_pane_id, ctx) {
        active_terminal.update(ctx, |view, ctx| view.on_pane_state_change(ctx));
    }
}

fn handle_terminal_view_event(
    group: &mut PaneGroup,
    terminal_pane_id: TerminalPaneId,
    event: &Event,
    ctx: &mut ViewContext<PaneGroup>,
) {
    let pane_id = terminal_pane_id.into();

    if group.pane_contents.contains_key(&pane_id) {
        match event {
            Event::Escape => ctx.emit(pane_group::Event::Escape),
            Event::ExecuteCommand(event) => {
                ctx.emit(pane_group::Event::ExecuteCommand(event.clone()));
            }
            Event::Exited => {
                // If the shell process exited before it successfully bootstrapped,
                // keep the pane open.  There might be useful information visible
                // in the output, and if this was the first shell spawned when the
                // user started the app, it will prevent it from suddenly quitting.
                if group
                    .terminal_view_from_pane_id(terminal_pane_id, ctx)
                    .is_some_and(|terminal_view| {
                        !terminal_view.as_ref(ctx).is_login_shell_bootstrapped()
                    })
                {
                    return;
                }

                group.close_pane(pane_id, ctx);
            }
            Event::CloseRequested => {
                group.close_pane_with_confirmation(pane_id, ctx);
            }
            Event::Pane(pane_event) => group.handle_pane_event(pane_id, pane_event, ctx),
            Event::BlockListCleared => {
                // Capture CMD-K to clear blocks here so we could remove
                // all the associated blocks stored in the history.
                if let Some(terminal_pane) = group.terminal_session_by_id(pane_id) {
                    terminal_pane.delete_blocks(ctx);
                }
            }
            Event::SendNotification(notification) => {
                ctx.emit(pane_group::Event::SendNotification {
                    notification: notification.clone(),
                    pane_id,
                })
            }
            Event::PluggableNotification { title, body } => {
                let message = if let Some(t) = title {
                    format!("{t}: {body}")
                } else {
                    body.clone()
                };
                ctx.emit(pane_group::Event::ShowToast {
                    message,
                    flavor: ToastFlavor::Default,
                    pane_id: Some(pane_id),
                })
            }
            Event::AppStateChanged => {
                ctx.emit(pane_group::Event::AppStateChanged);
            }
            Event::BlockCompleted { block, is_local } => {
                match group.terminal_session_by_id(pane_id) {
                    Some(pane) => {
                        if *GeneralSettings::as_ref(ctx).restore_session
                            && AppExecutionMode::as_ref(ctx).can_save_session()
                            && let Some(sender) = &group.model_event_sender
                        {
                            let block_completed_event = ModelEvent::SaveBlock(BlockCompleted {
                                pane_id: pane.session_uuid(),
                                block: block.clone(),
                                is_local: *is_local,
                            });

                            let sender_clone = sender.clone();
                            let _ = ctx.spawn(async move {
                                // Sending over a sync sender can block the current thread, so we do this async.
                                sender_clone.send(block_completed_event)
                            }, move |_, res, _| {
                                if let Err(err) = res {
                                    log::error!("Error sending block completed event for terminal id {terminal_pane_id:?} {err:?}");
                                }
                            });
                        }
                        ctx.emit(pane_group::Event::ActiveSessionChanged);
                    }
                    None => {
                        log::error!("Could not find uuid for terminal id: {terminal_pane_id:?}");
                    }
                };
            }
            Event::SessionBootstrapped => {
                ctx.emit(pane_group::Event::ActiveSessionChanged);
            }
            Event::OpenSettings(section) => {
                ctx.emit(pane_group::Event::OpenSettings(*section));
            }
            #[cfg(not(target_family = "wasm"))]
            Event::OpenPluginInstructionsPane(agent, kind) => {
                ctx.emit(pane_group::Event::OpenPluginInstructionsPane(*agent, *kind));
            }
            Event::AskAIAssistant(ask_type) => {
                ctx.emit(pane_group::Event::AskAIAssistant(ask_type.to_owned()))
            }
            Event::SyncInput(sync_event) => {
                if SyncedInputState::as_ref(ctx)
                    .should_sync_this_pane_group(ctx.view_id(), ctx.window_id())
                {
                    ctx.emit(pane_group::Event::SyncInput(sync_event.clone()));
                }
            }
            Event::ShowCommandSearch(options) => {
                ctx.emit(pane_group::Event::ShowCommandSearch(options.clone()));
            }
            Event::TerminalViewStateChanged => {
                ctx.emit(pane_group::Event::TerminalViewStateChanged);
            }
            Event::OnboardingTutorialCompleted => {
                ctx.emit(pane_group::Event::OnboardingTutorialCompleted);
            }
            Event::OpenWorkflowModalWithCommand(command) => {
                ctx.emit(pane_group::Event::OpenWorkflowModalWithCommand(
                    command.clone(),
                ));
            }
            Event::OpenWorkflowModalWithWorkflowObject(workflow_id) => {
                ctx.emit(pane_group::Event::OpenCloudWorkflowForEdit(*workflow_id));
            }
            Event::OpenWorkflowModalWithTemporary(workflow) => {
                ctx.emit(pane_group::Event::OpenWorkflowModalWithTemporary(
                    workflow.clone(),
                ));
            }
            Event::OpenPromptEditor => {
                ctx.emit(pane_group::Event::OpenPromptEditor);
            }
            Event::OpenAgentToolbarEditor => {
                ctx.emit(pane_group::Event::OpenAgentToolbarEditor);
            }
            Event::OpenCLIAgentToolbarEditor => {
                ctx.emit(pane_group::Event::OpenCLIAgentToolbarEditor);
            }
            Event::OpenFileInWarp { path, session } => {
                ctx.emit(pane_group::Event::OpenFileInWarp {
                    path: path.clone(),
                    session: session.clone(),
                });
            }
            #[cfg(feature = "local_fs")]
            Event::PreviewCodeInWarp { source } => {
                ctx.emit(pane_group::Event::PreviewCodeInWarp {
                    source: source.clone(),
                });
            }
            #[cfg(feature = "local_fs")]
            Event::OpenCodeInWarp { source, layout } => {
                ctx.emit(pane_group::Event::OpenCodeInWarp {
                    source: source.clone(),
                    layout: *layout,
                    line_col: if let CodeSource::Link { range_start, .. } = source {
                        *range_start
                    } else {
                        None
                    },
                });
            }
            Event::OpenCodeDiff { view } => {
                ctx.emit(pane_group::Event::OpenCodeDiff { view: view.clone() });
            }
            Event::OpenCodeReviewPane(arg) => {
                ctx.emit(pane_group::Event::OpenCodeReviewPane(arg.clone()));
            }
            Event::OpenCodeReviewPaneAndScrollToComment {
                open_code_review,
                comment,
                diff_mode,
            } => {
                ctx.emit(pane_group::Event::OpenCodeReviewPaneAndScrollToComment {
                    open_code_review: open_code_review.clone(),
                    comment: comment.clone(),
                    diff_mode: diff_mode.clone(),
                });
            }
            Event::ImportAllCodeReviewComments {
                open_code_review,
                comments,
                diff_mode,
            } => {
                ctx.emit(pane_group::Event::ImportAllCodeReviewComments {
                    open_code_review: open_code_review.clone(),
                    comments: comments.clone(),
                    diff_mode: diff_mode.clone(),
                });
            }
            Event::ToggleCodeReviewPane(arg) => {
                ctx.emit(pane_group::Event::ToggleCodeReviewPane(arg.clone()));
            }
            Event::FocusSession => {
                group.focus_pane(terminal_pane_id.into(), true, ctx);
                ctx.emit(pane_group::Event::FocusPaneGroup);
            }
            Event::ZapDriveObjectInPane(uid) => {
                ctx.emit(pane_group::Event::ZapDriveObjectInPane(uid.clone()));
            }
            Event::OpenSuggestedAgentModeWorkflowModal { workflow_and_id } => {
                ctx.emit(pane_group::Event::OpenSuggestedAgentModeWorkflowModal {
                    workflow_and_id: workflow_and_id.clone(),
                });
            }
            Event::OpenSuggestedRuleDialog { rule_and_id } => {
                ctx.emit(pane_group::Event::OpenSuggestedRuleModal {
                    rule_and_id: rule_and_id.clone(),
                });
            }
            Event::OpenAIFactCollection { sync_id } => {
                ctx.emit(pane_group::Event::OpenAIFactCollection { sync_id: *sync_id });
            }
            Event::SummarizationCancelDialogToggled { is_open } => {
                group.terminal_with_open_summarization_dialog = is_open.then_some(terminal_pane_id);
                ctx.notify();
            }
            // Zap Wave 7-3: the `Event::EnvironmentSetupModeSelectorToggled` handler
            // was physically removed along with the ambient-agent UI subsystem.
            #[cfg(feature = "local_fs")]
            Event::OpenFileWithTarget {
                path,
                target,
                line_col,
            } => {
                ctx.emit(pane_group::Event::OpenFileWithTarget {
                    path: path.clone(),
                    target: target.clone(),
                    line_col: *line_col,
                });
            }
            // Zap: forward the "open remote file" event emitted by the terminal through to pane_group → workspace.
            #[cfg(all(feature = "local_tty", feature = "local_fs"))]
            Event::OpenRemoteFileFromTerminal {
                remote_path,
                line_col,
            } => {
                ctx.emit(pane_group::Event::OpenRemoteFileFromTerminal {
                    remote_path: remote_path.clone(),
                    line_col: *line_col,
                });
            }
            Event::CopyFileToRemote { command, upload_id } => {
                let new_pane_id = group.insert_terminal_pane(
                    Direction::Right,
                    pane_id,
                    None, /*chosen_shell*/
                    ctx,
                );

                group.hide_pane_for_job(new_pane_id.into(), ctx);

                let new_terminal_view = group
                    .active_session_view(ctx)
                    .expect("should have new terminal view");
                new_terminal_view.update(ctx, |terminal_view, ctx| {
                    terminal_view.set_pending_command(command, ctx);
                    terminal_view.set_is_ssh_uploader(true);
                });

                ctx.emit(pane_group::Event::FileUploadCommand {
                    upload_id: *upload_id,
                    command: command.to_owned(),
                    remote_pane_id: terminal_pane_id,
                    local_pane_id: new_pane_id,
                });

                group.focus_pane(pane_id, true, ctx);
            }
            Event::FileUploadPasswordPending => {
                ctx.emit(pane_group::Event::FileUploadPasswordPending {
                    local_pane_id: terminal_pane_id,
                });
            }
            Event::OpenConversationHistory => {
                ctx.emit(OpenConversationHistory);
            }
            Event::FileUploadFinished(exit_code) => {
                ctx.emit(pane_group::Event::FileUploadFinished {
                    local_pane_id: terminal_pane_id,
                    exit_code: *exit_code,
                });

                // Each upload spawns its own new terminal pane. Once an upload
                // has finished, we know that its terminal session will no
                // longer be responsible for any UI-based uploads.
                if let Some(uploader_terminal_view) =
                    group.terminal_view_from_pane_id(terminal_pane_id, ctx)
                {
                    uploader_terminal_view.update(ctx, |terminal_view, _ctx| {
                        terminal_view.set_is_ssh_uploader(false);
                    });
                }
            }
            Event::OpenFileUploadSession(upload_id) => {
                ctx.emit(pane_group::Event::OpenFileUploadSession {
                    remote_pane_id: terminal_pane_id,
                    upload_id: *upload_id,
                })
            }
            Event::TerminateFileUploadSession(upload_id) => {
                ctx.emit(pane_group::Event::TerminateFileUploadSession {
                    remote_pane_id: terminal_pane_id,
                    upload_id: *upload_id,
                })
            }
            Event::OpenThemeChooser => {
                ctx.emit(pane_group::Event::OpenThemeChooser);
            }
            Event::OpenMCPSettingsPage { page } => {
                ctx.emit(pane_group::Event::OpenMCPSettingsPage { page: *page });
            }
            Event::OpenFilesPalette { source } => {
                ctx.emit(pane_group::Event::OpenFilesPalette { source: *source })
            }
            Event::OpenAddRulePane => {
                ctx.emit(crate::pane_group::Event::OpenAddRulePane);
            }
            Event::OpenRulesPane => {
                ctx.emit(crate::pane_group::Event::OpenAIFactCollection { sync_id: None });
            }
            Event::OpenAddPromptPane { initial_content } => {
                ctx.emit(crate::pane_group::Event::OpenAddPromptPane {
                    initial_content: initial_content.clone(),
                });
            }
            // Zap Wave 7-3: `OpenEnvironmentManagementPane` event forwarding was physically
            // removed along with the ambient-agent UI subsystem.
            #[cfg(feature = "local_fs")]
            Event::FileRenamed { old_path, new_path } => {
                ctx.emit(pane_group::Event::FileRenamed {
                    old_path: old_path.clone(),
                    new_path: new_path.clone(),
                });
            }
            #[cfg(feature = "local_fs")]
            Event::FileDeleted { path } => {
                ctx.emit(pane_group::Event::FileDeleted { path: path.clone() });
            }
            Event::ToggleLeftPanel {
                target_view,
                force_open,
            } => {
                ctx.emit(pane_group::Event::ToggleLeftPanel {
                    target_view: *target_view,
                    force_open: *force_open,
                });
            }
            Event::ToggleAIDocumentPane {
                document_id,
                document_version,
            } => {
                if let Some(conversation_id) =
                    crate::ai::document::ai_document_model::AIDocumentModel::as_ref(ctx)
                        .get_conversation_id_for_document_id(document_id)
                {
                    group.toggle_ai_document_pane(
                        conversation_id,
                        *document_id,
                        *document_version,
                        ctx,
                    );
                }
            }
            Event::HideAIDocumentPanes => {
                group.close_all_ai_document_panes(ctx);
            }
            Event::OpenAIDocumentPane {
                document_id,
                document_version,
                is_auto_open,
            } => {
                let should_open = if *is_auto_open {
                    // Auto-open: only open if there's already a visible plan pane
                    // (to replace it with the newest plan) or if there's enough space.
                    let has_visible_ai_doc_pane = group
                        .ai_document_panes()
                        .any(|pane_id| !group.is_pane_hidden_for_close(pane_id));

                    has_visible_ai_doc_pane
                        || group
                            .terminal_view_from_pane_id(terminal_pane_id, ctx)
                            .is_some_and(|tv| tv.as_ref(ctx).can_auto_open_panel())
                } else {
                    // User-triggered: always open.
                    true
                };

                if should_open
                    && let Some(conversation_id) =
                        crate::ai::document::ai_document_model::AIDocumentModel::as_ref(ctx)
                            .get_conversation_id_for_document_id(document_id)
                {
                    group.open_ai_document_pane(
                        conversation_id,
                        *document_id,
                        *document_version,
                        ctx,
                    );
                }
            }
            Event::OpenAgentProfileEditor { profile_id } => {
                ctx.emit(pane_group::Event::OpenAgentProfileEditor {
                    profile_id: profile_id.clone(),
                });
            }
            Event::InsertCodeReviewComments {
                repo_path,
                comments,
                diff_mode,
                open_code_review,
            } => {
                let LocalOrRemotePath::Local(repo_path) = repo_path else {
                    log::warn!("Ignoring code review comments for remote repository");
                    return;
                };
                ctx.emit(pane_group::Event::InsertCodeReviewComments {
                    repo_path: repo_path.clone(),
                    comments: comments.to_owned(),
                    diff_mode: diff_mode.to_owned(),
                    open_code_review: open_code_review.clone(),
                });
            }
            Event::StopAgentConversation { conversation_id } => {
                stop_agent_conversation(group, *conversation_id, ctx);
            }
            Event::KillAgentConversation { conversation_id } => {
                let source_terminal_view_id = group
                    .terminal_view_from_pane_id(terminal_pane_id, ctx)
                    .map(|terminal_view| terminal_view.id());
                kill_agent_conversation(group, source_terminal_view_id, *conversation_id, ctx);
            }
            Event::StartAgentConversation(request) => {
                dispatch_start_agent_conversation(
                    group,
                    pane_id,
                    terminal_pane_id,
                    request.clone(),
                    ctx,
                );
            }
            Event::RevealChildAgent { conversation_id } => {
                if let Some(&child_pane_id) = group.child_agent_panes.get(conversation_id) {
                    group.panes.show_pane_for_child_agent(child_pane_id);
                    group.handle_pane_count_change(ctx);
                    group.focus_pane(child_pane_id, true, ctx);
                } else {
                    log::warn!("No hidden pane found for child conversation {conversation_id:?}");
                }
            }
            Event::OpenChildAgentInNewTab { conversation_id } => {
                if group.child_agent_panes.contains_key(conversation_id) {
                    ctx.emit(pane_group::Event::OpenChildAgentInNewTab {
                        conversation_id: *conversation_id,
                    });
                } else {
                    log::warn!(
                        "OpenChildAgentInNewTab: no hidden pane found for conversation {conversation_id:?}"
                    );
                }
            }
            Event::OpenChildAgentInNewPane { conversation_id } => {
                if let Some(&child_pane_id) = group.child_agent_panes.get(conversation_id) {
                    group.panes.show_pane_for_child_agent(child_pane_id);
                    if let Some(child_terminal_view) =
                        group.terminal_view_from_pane_id(child_pane_id, ctx)
                    {
                        child_terminal_view.update(ctx, |view, ctx| {
                            view.mark_as_orchestration_split_off(ctx);
                        });
                    }
                    group.handle_pane_count_change(ctx);
                    group.focus_pane(child_pane_id, true, ctx);
                } else {
                    log::warn!(
                        "OpenChildAgentInNewPane: no hidden pane found for conversation {conversation_id:?}"
                    );
                }
            }
            _ => {}
        }
    } else {
        log::warn!("Session {terminal_pane_id:?} not found");
    }
}

fn apply_child_model_id_override(
    child_terminal_view_id: EntityId,
    model_id: Option<&str>,
    ctx: &mut ViewContext<PaneGroup>,
) {
    let Some(model_id) = model_id
        .map(str::trim)
        .filter(|model_id| !model_id.is_empty())
    else {
        return;
    };
    let llm_id: ai::LLMId = model_id.into();
    LLMPreferences::handle(ctx).update(ctx, |preferences, ctx| {
        preferences.set_agent_mode_llm_override(child_terminal_view_id, llm_id, ctx);
    });
}

fn dispatch_start_agent_conversation(
    group: &mut PaneGroup,
    parent_pane_id: PaneId,
    terminal_pane_id: TerminalPaneId,
    request: StartAgentRequest,
    ctx: &mut ViewContext<PaneGroup>,
) {
    let Some(parent_terminal_view_id) = group
        .terminal_view_from_pane_id(parent_pane_id, ctx)
        .map(|terminal_view| terminal_view.id())
    else {
        surface_start_agent_error(
            group,
            parent_pane_id,
            &request,
            None,
            "Failed to resolve the parent terminal for the local child agent.".to_string(),
            ctx,
        );
        return;
    };
    let Some(parent_agent_id) =
        ensure_local_parent_agent_id(request.parent_conversation_id, parent_terminal_view_id, ctx)
    else {
        surface_start_agent_error(
            group,
            parent_pane_id,
            &request,
            None,
            "Failed to resolve the parent conversation for the local child agent.".to_string(),
            ctx,
        );
        return;
    };

    match request.execution_mode.clone() {
        #[cfg(not(target_family = "wasm"))]
        StartAgentExecutionMode::Local {
            harness_type: None,
            model_id,
        } => launch_local_oz_child(group, parent_pane_id, request, model_id, ctx),
        #[cfg(not(target_family = "wasm"))]
        StartAgentExecutionMode::Local {
            harness_type: Some(harness_type),
            model_id,
        } => launch_local_harness_child(
            group,
            parent_pane_id,
            terminal_pane_id,
            request,
            harness_type,
            model_id,
            parent_agent_id,
            ctx,
        ),
        #[cfg(target_family = "wasm")]
        StartAgentExecutionMode::Local { .. } => surface_start_agent_error(
            group,
            parent_pane_id,
            &request,
            None,
            "Local child agents are not supported in WASM builds.".to_string(),
            ctx,
        ),
    }
}

fn ensure_local_parent_agent_id(
    parent_conversation_id: crate::ai::agent::conversation::AIConversationId,
    parent_terminal_view_id: EntityId,
    ctx: &mut ViewContext<PaneGroup>,
) -> Option<String> {
    let history = BlocklistAIHistoryModel::as_ref(ctx);
    let conversation = history.conversation(&parent_conversation_id)?;
    if let Some(agent_id) = conversation.agent_link_id() {
        return Some(agent_id);
    }

    let local_agent_id = parent_conversation_id.to_string();
    BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
        history.assign_run_id_for_conversation(
            parent_conversation_id,
            local_agent_id.clone(),
            None,
            parent_terminal_view_id,
            ctx,
        );
    });
    Some(local_agent_id)
}

fn surface_start_agent_error(
    group: &mut PaneGroup,
    parent_pane_id: PaneId,
    request: &StartAgentRequest,
    orchestration_harness: Option<Harness>,
    error_message: String,
    ctx: &mut ViewContext<PaneGroup>,
) {
    request.complete_error(error_message.clone());
    let _ = create_error_child_agent_conversation(
        group,
        ErrorChildAgentConversationRequest {
            parent_pane_id,
            name: normalize_orchestrator_agent_name(&request.name).unwrap_or_default(),
            parent_conversation_id: request.parent_conversation_id,
            orchestration_harness,
            error_message,
        },
        ctx,
    );
}

#[cfg(not(target_family = "wasm"))]
fn launch_local_oz_child(
    group: &mut PaneGroup,
    parent_pane_id: PaneId,
    request: StartAgentRequest,
    model_id: Option<String>,
    ctx: &mut ViewContext<PaneGroup>,
) {
    let Some(HiddenChildAgentConversation {
        terminal_view,
        terminal_view_id,
        conversation_id,
    }) = create_hidden_child_agent_conversation(
        group,
        HiddenChildAgentConversationRequest {
            parent_pane_id,
            name: normalize_orchestrator_agent_name(&request.name).unwrap_or_default(),
            parent_conversation_id: request.parent_conversation_id,
            orchestration_harness: Some(Harness::Oz),
            env_vars: HashMap::new(),
            task_context: None,
        },
        ctx,
    )
    else {
        surface_start_agent_error(
            group,
            parent_pane_id,
            &request,
            Some(Harness::Oz),
            "Failed to create a hidden pane for the local child agent.".to_string(),
            ctx,
        );
        return;
    };

    apply_child_model_id_override(terminal_view_id, model_id.as_deref(), ctx);
    let child_agent_id = conversation_id.to_string();
    BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
        history.assign_run_id_for_conversation(
            conversation_id,
            child_agent_id.clone(),
            None,
            terminal_view_id,
            ctx,
        );
    });
    terminal_view.update(ctx, |terminal_view, ctx| {
        terminal_view
            .ai_controller()
            .update(ctx, |controller, ctx| {
                controller.send_agent_query_in_conversation(
                    request.prompt.clone(),
                    conversation_id,
                    ctx,
                );
            });
        terminal_view.enter_agent_view(
            None,
            Some(conversation_id),
            AgentViewEntryOrigin::ChildAgent,
            ctx,
        );
    });
    request.complete_started(child_agent_id);
}

#[cfg(not(target_family = "wasm"))]
#[allow(clippy::too_many_arguments)]
fn launch_local_harness_child(
    group: &mut PaneGroup,
    parent_pane_id: PaneId,
    terminal_pane_id: TerminalPaneId,
    request: StartAgentRequest,
    harness_type: String,
    model_id: Option<String>,
    parent_agent_id: String,
    ctx: &mut ViewContext<PaneGroup>,
) {
    let startup_directory = group.startup_path_for_new_session(Some(terminal_pane_id), ctx);
    let shell_type = group
        .terminal_view_from_pane_id(parent_pane_id, ctx)
        .and_then(|terminal_view| terminal_view.as_ref(ctx).active_session_shell_type(ctx));
    let orchestration_harness =
        Harness::parse_orchestration_harness(&harness_type).unwrap_or(Harness::Unknown);
    let model_id_for_launch = model_id.clone();
    let agent_name = normalize_orchestrator_agent_name(&request.name);
    let startup_directory_for_task = startup_directory.clone();
    let prompt = request.prompt.clone();
    let _ = ctx.spawn(
        async move {
            prepare_local_harness_child_launch(
                prompt,
                harness_type,
                model_id_for_launch,
                Some(parent_agent_id),
                agent_name,
                shell_type,
                startup_directory,
            )
            .await
        },
        move |group, result, ctx| match result {
            Ok(PreparedLocalHarnessLaunch {
                command,
                env_vars,
                run_id,
                task_id,
            }) => {
                let Some(HiddenChildAgentConversation {
                    terminal_view,
                    terminal_view_id,
                    conversation_id,
                }) = create_hidden_child_agent_conversation(
                    group,
                    HiddenChildAgentConversationRequest {
                        parent_pane_id,
                        name: normalize_orchestrator_agent_name(&request.name).unwrap_or_default(),
                        parent_conversation_id: request.parent_conversation_id,
                        orchestration_harness: Some(orchestration_harness),
                        env_vars,
                        task_context: Some(HiddenChildAgentTaskContext {
                            task_id,
                            working_dir: startup_directory_for_task.clone(),
                        }),
                    },
                    ctx,
                )
                else {
                    surface_start_agent_error(
                        group,
                        parent_pane_id,
                        &request,
                        Some(orchestration_harness),
                        "Failed to create a hidden pane for the local child harness.".to_string(),
                        ctx,
                    );
                    return;
                };

                apply_child_model_id_override(terminal_view_id, model_id.as_deref(), ctx);
                let child_agent_id = run_id.clone();
                BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                    history.assign_run_id_for_conversation(
                        conversation_id,
                        run_id,
                        Some(task_id),
                        terminal_view_id,
                        ctx,
                    );
                });
                terminal_view.update(ctx, |terminal_view, ctx| {
                    terminal_view.execute_command_or_set_pending(&command, ctx);
                    terminal_view.enter_agent_view(
                        None,
                        Some(conversation_id),
                        AgentViewEntryOrigin::ChildAgent,
                        ctx,
                    );
                });
                request.complete_started(child_agent_id);
            }
            Err(error_message) => surface_start_agent_error(
                group,
                parent_pane_id,
                &request,
                Some(orchestration_harness),
                error_message,
                ctx,
            ),
        },
    );
}

#[cfg(feature = "local_fs")]
fn handle_ai_history_event(
    event: &BlocklistAIHistoryEvent,
    terminal_view_id: EntityId,
    terminal_pane_id: TerminalPaneId,
    model_event_sender: SyncSender<ModelEvent>,
    is_shared_ambient_agent_session: bool,
    ctx: &mut ViewContext<PaneGroup>,
) {
    use std::sync::Arc;

    use crate::ai::blocklist::{
        AIQueryHistoryOutputStatus, PersistedAIInput, PersistedAIInputType,
    };

    if event
        .terminal_surface_id()
        .is_some_and(|id| id != terminal_view_id)
    {
        return;
    }

    match event {
        BlocklistAIHistoryEvent::AppendedExchange {
            exchange_id,
            conversation_id,
            is_hidden,
            ..
        }
        | BlocklistAIHistoryEvent::UpdatedStreamingExchange {
            exchange_id,
            conversation_id,
            is_hidden,
            ..
        } => {
            // Check if session restoration is enabled.
            if !*GeneralSettings::as_ref(ctx).restore_session
                || !AppExecutionMode::as_ref(ctx).can_save_session()
            {
                return;
            }

            let Some(conversation) =
                BlocklistAIHistoryModel::as_ref(ctx).conversation(conversation_id)
            else {
                log::warn!("Received event with invalid conversation ID: {conversation_id:?}");
                return;
            };

            let Some(exchange) = conversation.exchange_with_id(*exchange_id) else {
                log::warn!("Received event with invalid exchange ID: {exchange_id:?}");
                return;
            };

            // Hidden blocks and passive-only conversations should not be restored, so we skip
            // them.
            if *is_hidden || conversation.is_entirely_passive() {
                return;
            }

            // Do not persist AI queries from shared ambient agent sessions that we've viewed,
            // as these were sent as part of an ambient agent run and shouldn't pollute the up arrow history.
            if is_shared_ambient_agent_session {
                return;
            }

            // Only query-bearing inputs (e.g. user queries) are persisted for
            // up-arrow history. Early return and skip writing for exchanges that
            // carry empty input.
            let inputs: Vec<_> = exchange
                .input
                .iter()
                .filter_map(|input| PersistedAIInputType::try_from(input).ok())
                .collect();
            if inputs.is_empty() {
                return;
            }

            let persisted_query = PersistedAIInput {
                start_ts: exchange.start_time,
                inputs,
                exchange_id: exchange.id,
                conversation_id: *conversation_id,
                output_status: AIQueryHistoryOutputStatus::from(&exchange.output_status),
                working_directory: exchange.working_directory.clone(),
                // TODO(CORE-3546): shell: exchange.shell.clone(),
                model_id: exchange.model_id.clone(),
                coding_model_id: exchange.coding_model_id.clone(),
            };
            let upsert_ai_query_event = ModelEvent::UpsertAIQuery {
                query: Arc::new(persisted_query),
            };
            let _ = ctx.spawn(
                // Sending over a sync sender can block the current thread, so we
                // do this async.
                async move { model_event_sender.send(upsert_ai_query_event) },
                move |_, res, _| {
                    if let Err(err) = res {
                        log::error!(
                            "Error sending upsert AI query event for terminal id {terminal_pane_id:?} {err:?}"
                        );
                    }
                },
            );
        }
        BlocklistAIHistoryEvent::ClearedConversationsForTerminalSurface { .. }
        | BlocklistAIHistoryEvent::ClearedActiveConversation { .. } => {
            ctx.emit(pane_group::Event::InvalidatedActiveConversation);
        }
        BlocklistAIHistoryEvent::RemoveConversation {
            conversation_id, ..
        } => {
            let conversation_id = conversation_id.to_string();
            // On remove, delete all related AI query and multi-agent conversation data for this conversation.
            let _ = ctx.spawn(
                async move {
                    model_event_sender.send(ModelEvent::DeleteAIConversation {
                        conversation_id: conversation_id.clone(),
                    })?;
                    model_event_sender.send(ModelEvent::DeleteMultiAgentConversations {
                        conversation_ids: vec![conversation_id],
                    })
                },
                |_, res, _| {
                    if let Err(err) = res {
                        log::error!("Error sending delete events for conversation: {err:?}");
                    }
                },
            );
        }
        // DeletedConversation SQL cleanup is handled directly in delete_conversation().
        BlocklistAIHistoryEvent::DeletedConversation { .. }
        | BlocklistAIHistoryEvent::StartedNewConversation { .. }
        | BlocklistAIHistoryEvent::UpdatedConversationStatus { .. }
        | BlocklistAIHistoryEvent::ReassignedExchange { .. }
        | BlocklistAIHistoryEvent::SetActiveConversation { .. }
        | BlocklistAIHistoryEvent::UpdatedTodoList { .. }
        | BlocklistAIHistoryEvent::UpdatedAutoexecuteOverride { .. }
        | BlocklistAIHistoryEvent::SplitConversation { .. }
        | BlocklistAIHistoryEvent::RestoredConversations { .. }
        | BlocklistAIHistoryEvent::CreatedSubtask { .. }
        | BlocklistAIHistoryEvent::UpgradedTask { .. }
        | BlocklistAIHistoryEvent::UpdatedConversationTitle { .. }
        | BlocklistAIHistoryEvent::UpdatedConversationMetadata { .. }
        | BlocklistAIHistoryEvent::UpdatedConversationArtifacts { .. }
        | BlocklistAIHistoryEvent::ConversationAgentIdAssigned { .. }
        | BlocklistAIHistoryEvent::ConversationTransferredBetweenTerminalSurfaces { .. }
        | BlocklistAIHistoryEvent::OrchestrationConfigUpdated { .. }
        | BlocklistAIHistoryEvent::ConversationUsageMetadataUpdated { .. }
        | BlocklistAIHistoryEvent::LocalSharedSessionEstablished { .. } => (),
    }
}
