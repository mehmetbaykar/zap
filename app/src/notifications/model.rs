//! Notification-center data model (Singleton).
//!
//! When 002ce467 cloud-removal deleted `agent_management` it cleared this model along with it, but
//! - completion/error notifications from the app's own BYOP agent (Oz)
//! - status notifications from third-party CLI agents (Claude / Codex / DeepSeek, etc.)
//!
//! still need to go through the notification center. This module is a slimmed-down version of the
//! pre-deletion `AgentNotificationsModel`:
//! - Removed the `ActiveAgentViewsModel` subscription (that model was the state source for the
//!   cloud-managed view, now deleted). It used to use `is_conversation_open` to decide "is the
//!   conversation view still open", changed to query `BlocklistAIHistoryModel::conversation()` to
//!   decide "is the conversation still in memory".
//! - Removed `AgentManagementEvent::ConversationNeedsAttention` (the legacy toast path, replaced by
//!   mailbox/toast_stack).
//! - Removed the legacy `should_trigger_notification` check (only the mailbox path is used).

use std::collections::HashMap;

use warp_core::features::FeatureFlag;
use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity, ViewHandle};

use crate::BlocklistAIHistoryModel;
use crate::ai::agent::conversation::{AIConversationId, ConversationStatus};
use crate::ai::artifacts::Artifact;
use crate::ai::blocklist::{BlocklistAIHistoryEvent, ConversationStatusUpdate, QueuedQueryModel};
use crate::notifications::item::{
    NotificationCategory, NotificationId, NotificationItem, NotificationItems, NotificationOrigin,
    NotificationSourceAgent,
};
use crate::settings::AISettings;
use crate::terminal::cli_agent_sessions::{
    CLIAgentSessionStatus, CLIAgentSessionsModel, CLIAgentSessionsModelEvent,
};
use crate::terminal::{CLIAgent, TerminalView};
use crate::workspace::util::is_terminal_view_in_same_tab;
use crate::workspace::{Workspace, WorkspaceRegistry};

/// The singleton model for the notification center:
/// - pushes notifications to the mailbox when the BYOP agent conversation state
///   (`BlocklistAIHistoryModel`) or CLI agent session state (`CLIAgentSessionsModel`) changes in a
///   key way;
/// - maintains `pending_artifacts` (the artifacts accumulated during each conversation's current
///   turn) and flushes them along with the notification at the terminal state.
pub struct NotificationsModel {
    notifications: NotificationItems,
    /// Artifacts accumulated during the current turn; drained into the notification at the terminal
    /// state (Success/Cancelled/Error), and cleared on InProgress.
    pub(crate) pending_artifacts: HashMap<AIConversationId, Vec<Artifact>>,
}

impl Entity for NotificationsModel {
    type Event = NotificationsEvent;
}

impl SingletonEntity for NotificationsModel {}

impl NotificationsModel {
    pub(crate) fn new(ctx: &mut ModelContext<Self>) -> Self {
        let history_model = BlocklistAIHistoryModel::handle(ctx);
        ctx.subscribe_to_model(&history_model, move |me, _, event, ctx| {
            me.handle_history_event(event, ctx);
        });

        let cli_sessions_model = CLIAgentSessionsModel::handle(ctx);
        ctx.subscribe_to_model(&cli_sessions_model, |me, _, event, ctx| {
            me.handle_cli_agent_session_event(event, ctx);
        });

        Self {
            notifications: NotificationItems::default(),
            pending_artifacts: HashMap::new(),
        }
    }

    pub(crate) fn notifications(&self) -> &NotificationItems {
        &self.notifications
    }

    pub(crate) fn mark_item_read(&mut self, id: NotificationId, ctx: &mut ModelContext<Self>) {
        if self.notifications.mark_item_read(id) {
            ctx.emit(NotificationsEvent::NotificationUpdated);
        }
    }

    pub(crate) fn mark_all_items_read(&mut self, ctx: &mut ModelContext<Self>) {
        if self.notifications.mark_all_items_read() {
            ctx.emit(NotificationsEvent::AllNotificationsMarkedRead);
        }
    }

    /// Mark all notifications on the given terminal view as read.
    pub(crate) fn mark_items_from_terminal_view_read(
        &mut self,
        terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        if !FeatureFlag::HOANotifications.is_enabled() {
            return;
        }
        if self
            .notifications
            .mark_all_terminal_view_items_as_read(terminal_view_id)
        {
            ctx.emit(NotificationsEvent::NotificationUpdated);
        }
    }

    fn handle_cli_agent_session_event(
        &mut self,
        event: &CLIAgentSessionsModelEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        if !FeatureFlag::HOANotifications.is_enabled() {
            return;
        }

        match event {
            CLIAgentSessionsModelEvent::Ended {
                terminal_view_id, ..
            } => {
                self.remove_notification_by_source(
                    NotificationOrigin::CLISession(*terminal_view_id),
                    ctx,
                );
            }
            CLIAgentSessionsModelEvent::Started { .. }
            | CLIAgentSessionsModelEvent::InputSessionChanged { .. }
            | CLIAgentSessionsModelEvent::SessionUpdated { .. } => {}
            CLIAgentSessionsModelEvent::StatusChanged {
                terminal_view_id,
                agent,
                status,
                session_context,
            } => match status {
                // The agent starts working again -> the previous notification is invalidated.
                CLIAgentSessionStatus::InProgress => {
                    self.remove_notification_by_source(
                        NotificationOrigin::CLISession(*terminal_view_id),
                        ctx,
                    );
                }
                CLIAgentSessionStatus::Success => {
                    let title = session_context
                        .display_title()
                        .unwrap_or_else(|| format!("{} completed", agent.display_name()));
                    let message = match agent {
                        CLIAgent::Codex => "Notification from Codex",
                        CLIAgent::DeepSeek => "Notification from DeepSeek",
                        CLIAgent::Antigravity => "Notification from Antigravity",
                        _ => "Task completed.",
                    };
                    let metadata = TerminalViewMetadata::lookup(*terminal_view_id, ctx);
                    self.add_notification(
                        title,
                        message.to_owned(),
                        NotificationCategory::Complete,
                        NotificationSourceAgent::CLI {
                            agent: *agent,
                            is_ambient: metadata.is_ambient,
                        },
                        NotificationOrigin::CLISession(*terminal_view_id),
                        *terminal_view_id,
                        vec![],
                        metadata.branch,
                        ctx,
                    );
                }
                CLIAgentSessionStatus::Blocked { message } => {
                    let title = session_context
                        .display_title()
                        .unwrap_or_else(|| format!("{} needs attention", agent.display_name()));
                    let metadata = TerminalViewMetadata::lookup(*terminal_view_id, ctx);
                    self.add_notification(
                        title,
                        message
                            .clone()
                            .unwrap_or_else(|| "Waiting for input.".to_owned()),
                        NotificationCategory::Request,
                        NotificationSourceAgent::CLI {
                            agent: *agent,
                            is_ambient: metadata.is_ambient,
                        },
                        NotificationOrigin::CLISession(*terminal_view_id),
                        *terminal_view_id,
                        vec![],
                        metadata.branch,
                        ctx,
                    );
                }
            },
        }
    }

    fn handle_history_event(
        &mut self,
        event: &BlocklistAIHistoryEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        // When the conversation is explicitly deleted / cleaned up as ephemeral, also clear its
        // notification and pending artifacts.
        if let BlocklistAIHistoryEvent::DeletedConversation {
            conversation_id, ..
        }
        | BlocklistAIHistoryEvent::RemoveConversation {
            conversation_id, ..
        } = event
        {
            if FeatureFlag::HOANotifications.is_enabled() {
                self.pending_artifacts.remove(conversation_id);
                self.remove_notification_by_source(
                    NotificationOrigin::Conversation(*conversation_id),
                    ctx,
                );
            }
            return;
        }

        // Accumulate artifacts as they arrive incrementally within a turn.
        if let BlocklistAIHistoryEvent::UpdatedConversationArtifacts {
            conversation_id,
            artifact,
            ..
        } = event
        {
            if FeatureFlag::HOANotifications.is_enabled() {
                self.pending_artifacts
                    .entry(*conversation_id)
                    .or_default()
                    .push(artifact.clone());
            }
            return;
        }

        let BlocklistAIHistoryEvent::UpdatedConversationStatus {
            terminal_surface_id,
            conversation_id,
            // Conversations restored at startup should not trigger a notification.
            update: ConversationStatusUpdate::Changed { .. },
            ..
        } = event
        else {
            return;
        };

        if !FeatureFlag::HOANotifications.is_enabled() {
            return;
        }

        let ai_history_model = BlocklistAIHistoryModel::as_ref(ctx);
        let Some(updated_conversation) = ai_history_model.conversation(conversation_id) else {
            return;
        };

        if updated_conversation.should_exclude_from_navigation()
            && !updated_conversation.is_child_agent_conversation()
        {
            return;
        }

        let status = updated_conversation.status().clone();
        let latest_query = updated_conversation.latest_user_query();
        self.handle_history_event_for_mailbox(
            &status,
            *conversation_id,
            latest_query,
            *terminal_surface_id,
            ctx,
        );
    }

    fn handle_history_event_for_mailbox(
        &mut self,
        status: &ConversationStatus,
        conversation_id: AIConversationId,
        latest_query: Option<String>,
        terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        let origin = NotificationOrigin::Conversation(conversation_id);

        let ai_history_model = BlocklistAIHistoryModel::as_ref(ctx);
        let conversation = ai_history_model.conversation(&conversation_id);
        let is_child = conversation.is_some_and(|c| c.is_child_agent_conversation());

        // For child conversations, check if the child's own conversation still exists in memory
        // (navigate directly) or if the parent conversation does (the child is visible via the
        // parent's ChildAgentStatusCard — navigate to the parent's pane). For non-child
        // conversations, just check whether the conversation itself still exists in memory.
        // This replaces the original `ActiveAgentViewsModel::is_conversation_open` check.
        let (is_open, effective_terminal_view_id, title) = if is_child {
            let child_open = conversation.is_some();
            let parent_open = !child_open
                && conversation
                    .and_then(|c| c.parent_conversation_id())
                    .is_some_and(|parent_id| ai_history_model.conversation(&parent_id).is_some());
            let nav_terminal_view_id = if child_open {
                terminal_view_id
            } else {
                conversation
                    .and_then(|c| c.parent_conversation_id())
                    .and_then(|parent_id| {
                        ai_history_model.terminal_surface_id_for_conversation(&parent_id)
                    })
                    .unwrap_or(terminal_view_id)
            };
            let child_name = conversation
                .and_then(|c| c.agent_name())
                .map(|name| name.to_owned())
                .or(latest_query)
                .unwrap_or_else(|| "Child agent".to_owned());
            (child_open || parent_open, nav_terminal_view_id, child_name)
        } else {
            let title = latest_query.unwrap_or_else(|| "Agent task".to_owned());
            (conversation.is_some(), terminal_view_id, title)
        };

        // The conversation no longer exists in memory (evicted / deleted) -> there is no navigable
        // target, so just clear the related notifications.
        if !is_open {
            self.pending_artifacts.remove(&conversation_id);
            self.remove_notification_by_source(origin, ctx);
            return;
        }

        let metadata = TerminalViewMetadata::lookup(effective_terminal_view_id, ctx);
        let oz_agent = NotificationSourceAgent::Oz {
            is_ambient: metadata.is_ambient,
        };

        match status {
            // When the agent resumes its work (or is automatically recovering from a
            // transient failure), clear stale notifications.
            ConversationStatus::InProgress
            | ConversationStatus::TransientError
            | ConversationStatus::WaitingForEvents => {
                self.remove_notification_by_source(origin, ctx);
            }
            ConversationStatus::Success => {
                // Suppress the completion notification when a queued follow-up prompt will
                // auto-send as soon as this conversation finishes. The conversation isn't
                // really in a stopped state, so the notification would be noisy. Pending
                // artifacts are left intact so they roll into the notification fired when the
                // conversation eventually finishes with an empty queue.
                if QueuedQueryModel::as_ref(ctx).has_autofireable_prompt(conversation_id) {
                    return;
                }
                let artifacts = self.flush_pending_artifacts(conversation_id);
                let message = if is_child {
                    "Child agent completed."
                } else {
                    "Task completed."
                };
                self.add_notification(
                    title,
                    message.to_owned(),
                    NotificationCategory::Complete,
                    oz_agent,
                    origin,
                    effective_terminal_view_id,
                    artifacts,
                    metadata.branch,
                    ctx,
                );
            }
            ConversationStatus::Cancelled => {
                let artifacts = self.flush_pending_artifacts(conversation_id);
                let message = if is_child {
                    "Child agent was cancelled."
                } else {
                    "Task was cancelled."
                };
                self.add_notification(
                    title,
                    message.to_owned(),
                    NotificationCategory::Complete,
                    oz_agent,
                    origin,
                    effective_terminal_view_id,
                    artifacts,
                    metadata.branch,
                    ctx,
                );
            }
            ConversationStatus::Blocked { blocked_action } => {
                self.add_notification(
                    title,
                    blocked_action.clone(),
                    NotificationCategory::Request,
                    oz_agent,
                    origin,
                    effective_terminal_view_id,
                    vec![],
                    metadata.branch,
                    ctx,
                );
            }
            ConversationStatus::Error => {
                let artifacts = self.flush_pending_artifacts(conversation_id);
                let message = if is_child {
                    "Child agent encountered an error."
                } else {
                    "Something went wrong."
                };
                self.add_notification(
                    title,
                    message.to_owned(),
                    NotificationCategory::Error,
                    oz_agent,
                    origin,
                    effective_terminal_view_id,
                    artifacts,
                    metadata.branch,
                    ctx,
                );
            }
        }
    }

    /// Remove the existing notification for the given source (if any) and emit an update event.
    fn remove_notification_by_source(
        &mut self,
        origin: NotificationOrigin,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.notifications.remove_by_origin(origin) {
            ctx.emit(NotificationsEvent::NotificationUpdated);
        }
    }

    /// Drain the artifacts accumulated during the given conversation's current turn.
    pub(crate) fn flush_pending_artifacts(
        &mut self,
        conversation_id: AIConversationId,
    ) -> Vec<Artifact> {
        self.pending_artifacts
            .remove(&conversation_id)
            .unwrap_or_default()
    }

    #[allow(clippy::too_many_arguments)]
    fn add_notification(
        &mut self,
        title: String,
        message: String,
        category: NotificationCategory,
        agent: NotificationSourceAgent,
        origin: NotificationOrigin,
        terminal_view_id: EntityId,
        artifacts: Vec<Artifact>,
        branch: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        if !*AISettings::as_ref(ctx).show_agent_notifications {
            return;
        }

        let is_visible = is_terminal_view_visible(terminal_view_id, ctx);
        let item = NotificationItem::new(
            title,
            message,
            category,
            agent,
            origin,
            is_visible,
            terminal_view_id,
            artifacts,
            branch,
        );

        let id = item.id;
        self.notifications.push(item);
        ctx.emit(NotificationsEvent::NotificationAdded { id });
    }
}

#[derive(Clone, Debug)]
pub enum NotificationsEvent {
    /// A notification was added to the notification center.
    NotificationAdded { id: NotificationId },
    /// A notification's read state changed.
    NotificationUpdated,
    /// All were marked as read.
    AllNotificationsMarkedRead,
}

impl ConversationStatus {
    /// Returns true if the updating the conversation with this status should trigger some
    /// notification to the user.
    ///
    /// Exhaustive match so a new `ConversationStatus` variant forces a
    /// deliberate decision about whether it should fire a notification.
    pub fn should_trigger_notification(&self) -> bool {
        match self {
            ConversationStatus::Success
            | ConversationStatus::Blocked { .. }
            | ConversationStatus::Error => true,
            // Streaming hasn't reached a notable state; a recovering or
            // yielded conversation is still active; user-cancellations are
            // self-evident.
            ConversationStatus::InProgress
            | ConversationStatus::TransientError
            | ConversationStatus::WaitingForEvents
            | ConversationStatus::Cancelled => false,
        }
    }
}

fn is_terminal_view_visible(terminal_view_id: EntityId, app: &AppContext) -> bool {
    let Some(active_id) = active_focused_terminal_id(app) else {
        return false;
    };
    active_id == terminal_view_id
        || is_terminal_view_in_same_tab(&active_id, &terminal_view_id, app)
}

/// Per-notification metadata derived from a single [`TerminalView`] lookup. Both fields
/// are read on the same emit path, so we resolve the view once and pass the projection
/// down rather than walking the workspace tree for each.
struct TerminalViewMetadata {
    is_ambient: bool,
    branch: Option<String>,
}

impl TerminalViewMetadata {
    fn lookup(terminal_view_id: EntityId, app: &AppContext) -> Self {
        let Some(terminal_view) = find_terminal_view_by_id(terminal_view_id, app) else {
            return Self {
                is_ambient: false,
                branch: None,
            };
        };
        let view = terminal_view.as_ref(app);
        Self {
            is_ambient: view.is_ambient_agent_session(app),
            branch: view.current_git_branch(app),
        }
    }
}

fn find_terminal_view_by_id(
    terminal_view_id: EntityId,
    app: &AppContext,
) -> Option<ViewHandle<TerminalView>> {
    for (_, workspace_handle) in WorkspaceRegistry::as_ref(app).all_workspaces(app) {
        for pane_group in workspace_handle.as_ref(app).tab_views() {
            let pane_group = pane_group.as_ref(app);
            for pane_id in pane_group.terminal_pane_ids() {
                if let Some(terminal_view) = pane_group.terminal_view_from_pane_id(pane_id, app)
                    && terminal_view.id() == terminal_view_id
                {
                    return Some(terminal_view);
                }
            }
        }
    }
    None
}

fn active_focused_terminal_id(app: &AppContext) -> Option<EntityId> {
    let active_window = app.windows().active_window()?;
    let workspace = app
        .views_of_type::<Workspace>(active_window)
        .and_then(|views| views.first().cloned())?;

    let workspace = workspace.as_ref(app);
    workspace.active_terminal_id(app)
}
