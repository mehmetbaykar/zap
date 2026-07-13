#[allow(dead_code)]
pub mod entry;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use clap::ValueEnum;
pub use entry::{
    AgentConversationEntry, AgentConversationEntryId, AgentConversationNavigationSubject,
    AgentConversationProvenance,
};
use futures::stream::AbortHandle;
use instant::Instant;
use itertools::Itertools;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use warp_cli::agent::Harness;
use warp_core::execution_mode::AppExecutionMode;
use warp_core::features::FeatureFlag;
use warp_core::report_error;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::WarpTheme;
use warpui::color::ColorU;
use warpui::r#async::Timer;
use warpui::windowing::{StateEvent, WindowManager};
use warpui::{
    duration_with_jitter, AppContext, Entity, EntityId, ModelContext, RequestState,
    SingletonEntity, WindowId,
};

use crate::ai::agent::api::ServerConversationToken;
use crate::ai::agent::conversation::{AIConversationId, ConversationStatus};
use crate::ai::ambient_agents::{
    AgentSource, AmbientAgentTask, AmbientAgentTaskId, AmbientAgentTaskState,
};
use crate::ai::artifacts::Artifact;
use crate::ai::blocklist::{
    format_credits, BlocklistAIHistoryEvent, BlocklistAIHistoryModel, ConversationStatusUpdate,
};
use crate::ai::conversation_navigation::ConversationNavigationData;
use crate::auth::{AuthStateProvider, UserUid};
use crate::ui_components::icons::Icon;
use crate::workspace::{RestoreConversationLayout, WorkspaceAction};
use crate::workspaces::user_profiles::UserProfiles;
const SESSION_EXPIRATION_TIME: chrono::Duration = chrono::Duration::weeks(1);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Available,
    Expired,
    Unavailable,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum StatusFilter {
    #[default]
    All,
    Working,
    Done,
    Failed,
}

impl StatusFilter {
    /// Returns `true` if a status transition from `prev_bucket` to `new_bucket` flips
    /// whether an item is included by this filter. `All` matches every bucket so it
    /// is never crossed; the other variants are crossed when exactly one of the buckets
    /// equals this filter.
    pub(crate) fn is_membership_crossed(
        self,
        prev_bucket: StatusFilter,
        new_bucket: StatusFilter,
    ) -> bool {
        match self {
            StatusFilter::All => false,
            StatusFilter::Working | StatusFilter::Done | StatusFilter::Failed => {
                (prev_bucket == self) != (new_bucket == self)
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum SourceFilter {
    #[default]
    All,
    Specific(AgentSource),
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum CreatorFilter {
    #[default]
    All,
    Specific {
        name: String,
        uid: String,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ArtifactFilter {
    #[default]
    All,
    PullRequest,
    Plan,
    Screenshot,
    File,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum CreatedOnFilter {
    #[default]
    All,
    Last24Hours,
    Past3Days,
    LastWeek,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum EnvironmentFilter {
    #[default]
    All,
    NoEnvironment,
    Specific(String),
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnerFilter {
    All,
    #[default]
    PersonalOnly,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum HarnessFilter {
    #[default]
    All,
    Specific(Harness),
}

impl Serialize for HarnessFilter {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            HarnessFilter::All => serializer.serialize_str("all"),
            HarnessFilter::Specific(harness) => serializer.collect_str(harness),
        }
    }
}

impl<'de> Deserialize<'de> for HarnessFilter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(Harness::from_str(&raw, false)
            .ok()
            .map(HarnessFilter::Specific)
            .unwrap_or(HarnessFilter::All))
    }
}

#[derive(Default, PartialEq, Eq, Clone, Debug, Serialize, Deserialize)]
pub struct AgentManagementFilters {
    pub owners: OwnerFilter,
    pub status: StatusFilter,
    pub source: SourceFilter,
    pub created_on: CreatedOnFilter,
    pub creator: CreatorFilter,
    pub artifact: ArtifactFilter,
    #[serde(default)]
    pub environment: EnvironmentFilter,
    #[serde(default)]
    pub harness: HarnessFilter,
}

impl AgentManagementFilters {
    pub fn reset_all_but_owner(&mut self) {
        self.status = StatusFilter::default();
        self.source = SourceFilter::default();
        self.created_on = CreatedOnFilter::default();
        self.creator = CreatorFilter::default();
        self.artifact = ArtifactFilter::default();
        self.environment = EnvironmentFilter::default();
        self.harness = HarnessFilter::default();
    }

    pub fn is_filtering(&self) -> bool {
        self.status != StatusFilter::default()
            || self.source != SourceFilter::default()
            || self.created_on != CreatedOnFilter::default()
            || self.creator != CreatorFilter::default() && self.owners != OwnerFilter::PersonalOnly
            || self.artifact != ArtifactFilter::default()
            || self.environment != EnvironmentFilter::default()
            || self.harness != HarnessFilter::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentRunDisplayStatus {
    /// Raw task-service lifecycle states. `from_task` only returns `TaskInProgress` while the
    /// task still has an active execution, or when there is no shadowed local conversation to
    /// provide a more granular status.
    TaskQueued,
    TaskPending,
    TaskClaimed,
    TaskInProgress,
    TaskSucceeded,
    TaskFailed,
    TaskError,
    TaskBlocked {
        blocked_action: String,
    },
    TaskCancelled,
    TaskUnknown,
    /// Conversation-derived lifecycle states, used for interactive conversations and for
    /// in-progress ambient tasks after they can be resolved to their shadowed local conversation.
    ConversationInProgress,
    ConversationSucceeded,
    ConversationError,
    ConversationBlocked {
        blocked_action: String,
    },
    ConversationCancelled,
}

impl AgentRunDisplayStatus {
    pub fn from_task(task: &AmbientAgentTask, app: &AppContext) -> Self {
        match &task.state {
            AmbientAgentTaskState::Queued
            | AmbientAgentTaskState::Pending
            | AmbientAgentTaskState::Claimed => Self::from_task_state(task),
            AmbientAgentTaskState::InProgress => {
                if task.has_active_execution() {
                    return Self::from_task_state(task);
                }
                let history_model = BlocklistAIHistoryModel::as_ref(app);
                entry::conversation_id_shadowed_by_task(task, history_model)
                    .and_then(|conversation_id| history_model.conversation(&conversation_id))
                    .map(|conversation| Self::from_conversation_status(conversation.status()))
                    .unwrap_or_else(|| Self::from_task_state(task))
            }
            AmbientAgentTaskState::Succeeded
            | AmbientAgentTaskState::Failed
            | AmbientAgentTaskState::Error
            | AmbientAgentTaskState::Blocked
            | AmbientAgentTaskState::Cancelled
            | AmbientAgentTaskState::Unknown => Self::from_task_state(task),
        }
    }

    pub fn from_conversation_status(status: &ConversationStatus) -> Self {
        match status {
            ConversationStatus::InProgress => Self::ConversationInProgress,
            ConversationStatus::Success => Self::ConversationSucceeded,
            ConversationStatus::Error => Self::ConversationError,
            ConversationStatus::Cancelled => Self::ConversationCancelled,
            ConversationStatus::Blocked { blocked_action } => Self::ConversationBlocked {
                blocked_action: blocked_action.clone(),
            },
        }
    }

    fn from_task_state(task: &AmbientAgentTask) -> Self {
        match &task.state {
            AmbientAgentTaskState::Queued => Self::TaskQueued,
            AmbientAgentTaskState::Pending => Self::TaskPending,
            AmbientAgentTaskState::Claimed => Self::TaskClaimed,
            AmbientAgentTaskState::InProgress => Self::TaskInProgress,
            AmbientAgentTaskState::Succeeded => Self::TaskSucceeded,
            AmbientAgentTaskState::Failed => Self::TaskFailed,
            AmbientAgentTaskState::Error => Self::TaskError,
            AmbientAgentTaskState::Blocked => Self::TaskBlocked {
                blocked_action: task
                    .status_message
                    .as_ref()
                    .map(|m| m.message.clone())
                    .unwrap_or_else(|| "Task blocked".to_string()),
            },
            AmbientAgentTaskState::Cancelled => Self::TaskCancelled,
            AmbientAgentTaskState::Unknown => Self::TaskUnknown,
        }
    }

    pub fn status_filter(&self) -> StatusFilter {
        match self {
            AgentRunDisplayStatus::TaskQueued
            | AgentRunDisplayStatus::TaskPending
            | AgentRunDisplayStatus::TaskClaimed
            | AgentRunDisplayStatus::TaskInProgress
            | AgentRunDisplayStatus::ConversationInProgress => StatusFilter::Working,
            AgentRunDisplayStatus::TaskSucceeded | AgentRunDisplayStatus::ConversationSucceeded => {
                StatusFilter::Done
            }
            AgentRunDisplayStatus::TaskFailed
            | AgentRunDisplayStatus::TaskError
            | AgentRunDisplayStatus::TaskBlocked { .. }
            | AgentRunDisplayStatus::TaskCancelled
            | AgentRunDisplayStatus::TaskUnknown
            | AgentRunDisplayStatus::ConversationError
            | AgentRunDisplayStatus::ConversationBlocked { .. }
            | AgentRunDisplayStatus::ConversationCancelled => StatusFilter::Failed,
        }
    }

    pub fn to_conversation_status(&self) -> ConversationStatus {
        match self {
            AgentRunDisplayStatus::TaskQueued
            | AgentRunDisplayStatus::TaskPending
            | AgentRunDisplayStatus::TaskClaimed
            | AgentRunDisplayStatus::TaskInProgress
            | AgentRunDisplayStatus::ConversationInProgress => ConversationStatus::InProgress,
            AgentRunDisplayStatus::TaskSucceeded | AgentRunDisplayStatus::ConversationSucceeded => {
                ConversationStatus::Success
            }
            AgentRunDisplayStatus::TaskFailed
            | AgentRunDisplayStatus::TaskError
            | AgentRunDisplayStatus::TaskUnknown
            | AgentRunDisplayStatus::ConversationError => ConversationStatus::Error,
            AgentRunDisplayStatus::TaskBlocked { blocked_action }
            | AgentRunDisplayStatus::ConversationBlocked { blocked_action } => {
                ConversationStatus::Blocked {
                    blocked_action: blocked_action.clone(),
                }
            }
            AgentRunDisplayStatus::TaskCancelled | AgentRunDisplayStatus::ConversationCancelled => {
                ConversationStatus::Cancelled
            }
        }
    }

    pub fn is_cancellable(&self) -> bool {
        self.is_working()
    }

    pub fn is_working(&self) -> bool {
        matches!(
            self,
            AgentRunDisplayStatus::TaskQueued
                | AgentRunDisplayStatus::TaskPending
                | AgentRunDisplayStatus::TaskClaimed
                | AgentRunDisplayStatus::TaskInProgress
                | AgentRunDisplayStatus::ConversationInProgress
        )
    }

    pub fn status_icon_and_color(&self, theme: &WarpTheme) -> (Icon, ColorU) {
        match self {
            AgentRunDisplayStatus::TaskQueued
            | AgentRunDisplayStatus::TaskPending
            | AgentRunDisplayStatus::TaskClaimed
            | AgentRunDisplayStatus::TaskInProgress
            | AgentRunDisplayStatus::ConversationInProgress => {
                (Icon::ClockLoader, theme.ansi_fg_magenta())
            }
            AgentRunDisplayStatus::TaskSucceeded | AgentRunDisplayStatus::ConversationSucceeded => {
                (Icon::Check, theme.ansi_fg_green())
            }
            AgentRunDisplayStatus::TaskFailed
            | AgentRunDisplayStatus::TaskError
            | AgentRunDisplayStatus::TaskUnknown
            | AgentRunDisplayStatus::ConversationError => (Icon::Triangle, theme.ansi_fg_red()),
            AgentRunDisplayStatus::TaskBlocked { .. }
            | AgentRunDisplayStatus::ConversationBlocked { .. } => {
                (Icon::StopFilled, theme.ansi_fg_yellow())
            }
            AgentRunDisplayStatus::TaskCancelled => (
                Icon::Cancelled,
                theme.disabled_text_color(theme.background()).into_solid(),
            ),
            AgentRunDisplayStatus::ConversationCancelled => {
                (Icon::StopFilled, internal_colors::neutral_5(theme))
            }
        }
    }
}

impl std::fmt::Display for AgentRunDisplayStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentRunDisplayStatus::TaskQueued => write!(f, "Queued"),
            AgentRunDisplayStatus::TaskPending => write!(f, "Pending"),
            AgentRunDisplayStatus::TaskClaimed => write!(f, "Claimed"),
            AgentRunDisplayStatus::TaskInProgress
            | AgentRunDisplayStatus::ConversationInProgress => write!(f, "In progress"),
            AgentRunDisplayStatus::TaskSucceeded | AgentRunDisplayStatus::ConversationSucceeded => {
                write!(f, "Done")
            }
            AgentRunDisplayStatus::TaskFailed => write!(f, "Failed"),
            AgentRunDisplayStatus::TaskError | AgentRunDisplayStatus::ConversationError => {
                write!(f, "Error")
            }
            AgentRunDisplayStatus::TaskBlocked { .. }
            | AgentRunDisplayStatus::ConversationBlocked { .. } => write!(f, "Blocked"),
            AgentRunDisplayStatus::TaskCancelled | AgentRunDisplayStatus::ConversationCancelled => {
                write!(f, "Cancelled")
            }
            AgentRunDisplayStatus::TaskUnknown => write!(f, "Failed"),
        }
    }
}

/// Stores conversation metadata needed for display in conversation/task views.
pub struct ConversationMetadata {
    pub nav_data: ConversationNavigationData,
}

/// ConversationOrTask is a wrapper around either conversation
/// or task data stored in the `AgentConversationsModel`.
///
/// It provides a unified interface for reading data related to tasks and conversations.
pub enum ConversationOrTask<'a> {
    Task(&'a AmbientAgentTask),
    Conversation(&'a ConversationMetadata),
}

impl ConversationOrTask<'_> {
    pub fn title(&self, app: &AppContext) -> String {
        match self {
            ConversationOrTask::Task(task) => task.title.clone(),
            ConversationOrTask::Conversation(metadata) => {
                // We try to read the title from the history model first (that's the most up-to-date),
                // but fall back to the one stored in the navigation data.
                let history_model = BlocklistAIHistoryModel::as_ref(app);
                history_model
                    .conversation(&metadata.nav_data.id)
                    .and_then(|conv| conv.title().clone())
                    .unwrap_or(metadata.nav_data.title.clone())
            }
        }
    }

    /// Map to conversation status for the UI status display
    pub fn status(&self, app: &AppContext) -> ConversationStatus {
        match self {
            ConversationOrTask::Task(task) => match &task.state {
                AmbientAgentTaskState::Queued
                | AmbientAgentTaskState::Pending
                | AmbientAgentTaskState::Claimed
                | AmbientAgentTaskState::InProgress => ConversationStatus::InProgress,
                AmbientAgentTaskState::Succeeded => ConversationStatus::Success,
                AmbientAgentTaskState::Cancelled => ConversationStatus::Cancelled,
                AmbientAgentTaskState::Blocked => ConversationStatus::Blocked {
                    blocked_action: task
                        .status_message
                        .as_ref()
                        .map(|m| m.message.clone())
                        .unwrap_or_else(|| "Task blocked".to_string()),
                },
                AmbientAgentTaskState::Failed
                | AmbientAgentTaskState::Error
                | AmbientAgentTaskState::Unknown => ConversationStatus::Error,
            },
            ConversationOrTask::Conversation(metadata) => {
                let history_model = BlocklistAIHistoryModel::as_ref(app);
                history_model
                    .conversation(&metadata.nav_data.id)
                    .map(|conv| conv.status().clone())
                    .unwrap_or(ConversationStatus::Success)
            }
        }
    }

    pub fn display_status(&self, app: &AppContext) -> AgentRunDisplayStatus {
        match self {
            ConversationOrTask::Task(task) => AgentRunDisplayStatus::from_task(task, app),
            ConversationOrTask::Conversation(metadata) => {
                let history_model = BlocklistAIHistoryModel::as_ref(app);
                history_model
                    .conversation(&metadata.nav_data.id)
                    .map(|conv| AgentRunDisplayStatus::from_conversation_status(conv.status()))
                    .unwrap_or(AgentRunDisplayStatus::ConversationSucceeded)
            }
        }
    }

    /// Grab the creator name from the task, or from the auth state if it is a conversation
    pub fn creator_name(&self, app: &AppContext) -> Option<String> {
        match self {
            ConversationOrTask::Task(task) => task.creator_display_name().or_else(|| {
                // Fallback to the cached users in the UserProfiles singleton
                let uid = task.creator.as_ref().map(|c| &c.uid)?;
                let user_profiles = UserProfiles::as_ref(app);
                user_profiles.displayable_identifier_for_uid(UserUid::new(uid))
            }),
            ConversationOrTask::Conversation(_) => {
                AuthStateProvider::as_ref(app).get().username_for_display()
            }
        }
    }

    /// Grab the creator UID from the task, or from the auth state if it is a conversation
    pub fn creator_uid(&self, app: &AppContext) -> Option<String> {
        match self {
            ConversationOrTask::Task(task) => task.creator.as_ref().map(|c| c.uid.clone()),
            ConversationOrTask::Conversation(_) => AuthStateProvider::as_ref(app)
                .get()
                .user_id()
                .map(|uid| uid.to_string()),
        }
    }

    /// Returns the request usage for the task or conversation
    pub(super) fn request_usage(&self, app: &AppContext) -> Option<f32> {
        match self {
            ConversationOrTask::Task(task) => task.credits_used(),
            ConversationOrTask::Conversation(metadata) => {
                let history_model = BlocklistAIHistoryModel::as_ref(app);
                history_model
                    .conversation(&metadata.nav_data.id)
                    .map(|conv| conv.credits_spent())
                    .or_else(|| {
                        history_model
                            .get_conversation_metadata(&metadata.nav_data.id)
                            .and_then(|m| m.credits_spent)
                    })
            }
        }
    }

    /// Formats the request usage for display.
    pub fn display_request_usage(&self, app: &AppContext) -> Option<String> {
        self.request_usage(app).map(format_credits)
    }

    pub fn last_updated(&self) -> DateTime<Utc> {
        match self {
            ConversationOrTask::Task(task) => task.updated_at,
            ConversationOrTask::Conversation(metadata) => metadata.nav_data.last_updated.into(),
        }
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        match self {
            ConversationOrTask::Task(task) => task.created_at,
            ConversationOrTask::Conversation(metadata) => metadata.nav_data.last_updated.into(),
        }
    }

    pub fn is_ambient_agent_conversation(&self) -> bool {
        matches!(self, ConversationOrTask::Task(_))
    }

    /// Returns the navigation data for local conversations, used for emitting the Navigate event.
    pub fn navigation_data(&self) -> Option<&ConversationNavigationData> {
        match self {
            ConversationOrTask::Task(_) => None,
            ConversationOrTask::Conversation(metadata) => Some(&metadata.nav_data),
        }
    }

    pub fn run_time(&self) -> Option<String> {
        match self {
            // TODO this should really be done server-side
            ConversationOrTask::Task(task) => {
                let Some(duration) = task.run_time() else {
                    return Some("Not started".to_string());
                };
                if duration.num_minutes() < 1 {
                    Some(format!("{} seconds", duration.num_seconds()))
                } else {
                    Some(format!("{} minutes", duration.num_minutes()))
                }
            }
            // Local conversations don't currently track run time
            ConversationOrTask::Conversation(_) => None,
        }
    }

    pub fn source(&self) -> Option<&AgentSource> {
        match self {
            ConversationOrTask::Task(task) => task.source.as_ref(),
            ConversationOrTask::Conversation(_) => Some(&AgentSource::Interactive),
        }
    }

    pub fn environment_id(&self) -> Option<&str> {
        match self {
            ConversationOrTask::Task(task) => task
                .agent_config_snapshot
                .as_ref()
                .and_then(|s| s.environment_id.as_deref()),
            ConversationOrTask::Conversation(_) => None,
        }
    }

    /// Resolve the effective execution harness for this run.
    pub fn harness(&self) -> Option<Harness> {
        match self {
            ConversationOrTask::Task(task) => {
                task.agent_config_snapshot.as_ref().and_then(|config| {
                    config
                        .harness
                        .as_ref()
                        .map(|h| h.harness_type)
                        .or(Some(Harness::Oz))
                })
            }
            ConversationOrTask::Conversation(_) => Some(Harness::Oz),
        }
    }

    /// Returns artifacts for the task or conversation.
    pub fn artifacts(&self, app: &AppContext) -> Vec<Artifact> {
        match self {
            ConversationOrTask::Task(task) => task.artifacts.clone(),
            ConversationOrTask::Conversation(metadata) => {
                let history_model = BlocklistAIHistoryModel::as_ref(app);
                history_model
                    .conversation(&metadata.nav_data.id)
                    .map(|conv| conv.artifacts().to_vec())
                    .or_else(|| {
                        history_model
                            .get_conversation_metadata(&metadata.nav_data.id)
                            .map(|m| m.artifacts.clone())
                    })
                    .unwrap_or_default()
            }
        }
    }

    pub fn get_session_status(&self) -> Option<SessionStatus> {
        match self {
            ConversationOrTask::Task(task) => {
                if task.session_id.is_some() {
                    Some(SessionStatus::Available)
                } else if (Utc::now() - task.created_at) > SESSION_EXPIRATION_TIME {
                    Some(SessionStatus::Expired)
                } else {
                    Some(SessionStatus::Unavailable)
                }
            }
            ConversationOrTask::Conversation(_) => None,
        }
    }

    /// Check if this item matches the current status filter.
    fn matches_status(&self, status_filter: &StatusFilter, app: &AppContext) -> bool {
        match status_filter {
            StatusFilter::All => true,
            StatusFilter::Working | StatusFilter::Done | StatusFilter::Failed => {
                self.display_status(app).status_filter() == *status_filter
            }
        }
    }

    /// Check if this item matches the artifact filter.
    fn matches_artifact(&self, artifact_filter: &ArtifactFilter, app: &AppContext) -> bool {
        artifacts_match_filter(&self.artifacts(app), artifact_filter)
    }

    /// Check if this item matches the harness filter.
    fn matches_harness(&self, harness_filter: &HarnessFilter) -> bool {
        match harness_filter {
            HarnessFilter::All => true,
            HarnessFilter::Specific(h) => self.harness() == Some(*h),
        }
    }

    /// Check if this item matches the owner and creator filters.
    fn matches_owner_and_creator(
        &self,
        owner_filter: &OwnerFilter,
        creator_filter: &CreatorFilter,
        app: &AppContext,
    ) -> bool {
        let current_user_id = AuthStateProvider::as_ref(app)
            .get()
            .user_id()
            .map(|uid| uid.as_string());

        // First check owner filter
        let passes_owner = match owner_filter {
            OwnerFilter::All => true,
            OwnerFilter::PersonalOnly => match self {
                ConversationOrTask::Task(_) => self.creator_uid(app) == current_user_id,
                // Local conversations are always owned by the current user
                ConversationOrTask::Conversation(_) => true,
            },
        };

        if !passes_owner {
            return false;
        }

        // We don't want to apply the creator filter if we are in the personal only view.
        if matches!(owner_filter, OwnerFilter::PersonalOnly) {
            return true;
        }

        // Then check creator filter (only relevant when owner is "All")
        match creator_filter {
            CreatorFilter::All => true,
            CreatorFilter::Specific { name, .. } => self.creator_name(app).as_ref() == Some(name),
        }
    }

    /// Returns the appropriate `WorkspaceAction` to dispatch when opening this item.
    /// This encapsulates the decision logic for opening ambient agent runs vs
    /// navigating to local conversations.
    pub fn get_open_action(
        &self,
        restore_layout: Option<RestoreConversationLayout>,
    ) -> Option<WorkspaceAction> {
        match self {
            ConversationOrTask::Task(_) => None,
            ConversationOrTask::Conversation(metadata) => {
                let nav_data = &metadata.nav_data;
                Some(WorkspaceAction::RestoreOrNavigateToConversation {
                    conversation_id: nav_data.id,
                    window_id: nav_data.window_id,
                    pane_view_locator: nav_data.pane_view_locator,
                    terminal_view_id: nav_data.terminal_view_id,
                    restore_layout,
                })
            }
        }
    }
}

pub(crate) fn artifacts_match_filter(
    artifacts: &[Artifact],
    artifact_filter: &ArtifactFilter,
) -> bool {
    match artifact_filter {
        ArtifactFilter::All => true,
        ArtifactFilter::PullRequest => artifacts
            .iter()
            .any(|artifact| matches!(artifact, Artifact::PullRequest { .. })),
        ArtifactFilter::Plan => artifacts
            .iter()
            .any(|artifact| matches!(artifact, Artifact::Plan { .. })),
        ArtifactFilter::Screenshot => artifacts
            .iter()
            .any(|artifact| matches!(artifact, Artifact::Screenshot { .. })),
        ArtifactFilter::File => artifacts
            .iter()
            .any(|artifact| matches!(artifact, Artifact::File { .. })),
    }
}

/// This model serves as a unified interface for reading both local and ambient agent conversations
/// (i.e. conversations & tasks). The model is responsible for polling for new tasks and updating
/// its local state accordingly.
///
/// This model backs both the agent management view and the conversation list view.
pub struct AgentConversationsModel {
    /// A map of task IDs to agent tasks.
    tasks: HashMap<AmbientAgentTaskId, AmbientAgentTask>,
    /// A map of conversation IDs to local conversations.
    conversations: HashMap<AIConversationId, ConversationMetadata>,
    /// Set of view IDs actively consuming this model's data per window.
    /// Zap: after localization there is no polling; this is only kept as a placeholder record for register_view_open/closed.
    active_data_consumers_per_window: HashMap<WindowId, HashSet<EntityId>>,
    /// Whether we have finished the initial task load
    has_finished_initial_load: bool,
    /// Task IDs that have been manually opened from the management page.
    /// These will appear in the conversation list even if their source is not user-initiated
    /// (and even after they have been closed).
    manually_opened_task_ids: HashSet<AmbientAgentTaskId>,
}

pub enum AgentConversationsModelEvent {
    /// Initial load of tasks completed.
    ConversationsLoaded,
    /// Existing task data may have been updated (e.g., state changes).
    TasksUpdated,
    /// Conversation status data was updated
    ConversationUpdated { kind: ConversationUpdateKind },
    /// Conversation artifacts were updated (plans, PRs, etc.)
    ConversationArtifactsUpdated { conversation_id: AIConversationId },
    /// A task was manually opened from the management page.
    TaskManuallyOpened,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationUpdateKind {
    /// The conversation was re-loaded into a terminal view.
    Restored,
    /// The conversation's status was set.
    StatusSet {
        prev_filter: StatusFilter,
        new_filter: StatusFilter,
    },
    /// Conversation metadata or capabilities changed.
    MetadataChanged,
}

impl Entity for AgentConversationsModel {
    type Event = AgentConversationsModelEvent;
}

impl SingletonEntity for AgentConversationsModel {}

impl AgentConversationsModel {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        // Zap (localization, Phase 3b-1 / Wave 6-6): AgentConversationsModel originally polled/probed
        // remote ambient agent tasks and conversation metadata. In the localized scenario:
        //   - there is no polling subsystem (physically removed in Wave 6-6)
        //   - has_finished_initial_load is directly true, so UI queries return empty collections
        // BYOP agent local runs do not depend on this model.
        //
        // Issue #93 fix: we must subscribe to BlocklistAIHistoryModel events, otherwise after the user
        // deletes a conversation in the history list, this model's cached conversations won't refresh and the UI will keep showing the deleted item.
        let history_model = BlocklistAIHistoryModel::handle(ctx);
        ctx.subscribe_to_model(&history_model, |me, event, ctx| {
            me.handle_history_event(event, ctx);
        });

        Self {
            tasks: HashMap::new(),
            conversations: HashMap::new(),
            active_data_consumers_per_window: HashMap::new(),
            has_finished_initial_load: true,
            manually_opened_task_ids: HashSet::new(),
        }
    }

    pub fn is_loading(&self) -> bool {
        !self.has_finished_initial_load
    }

    /// Sync all conversations to the AgentConversationsModel.
    ///
    /// This function will loop through all active panes, recently closed panes, and historical
    /// conversations to construct a complete snapshot of conversations.
    pub fn sync_conversations(&mut self, ctx: &mut ModelContext<Self>) {
        if !FeatureFlag::InteractiveConversationManagementView.is_enabled() {
            return;
        }

        let nav_data_list = ConversationNavigationData::all_conversations(ctx);

        self.conversations.clear();
        for nav_data in nav_data_list {
            let conversation_id = nav_data.id;
            let metadata = ConversationMetadata { nav_data };
            self.conversations.insert(conversation_id, metadata);
        }

        ctx.emit(AgentConversationsModelEvent::ConversationsLoaded);
    }

    /// Called when a view that consumes this model's data becomes visible.
    /// Uses view_id to make registration idempotent.
    pub fn register_view_open(
        &mut self,
        window_id: WindowId,
        view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.active_data_consumers_per_window
            .entry(window_id)
            .or_default()
            .insert(view_id);
        self.sync_conversations(ctx);
    }

    /// Called when a view that consumes this model's data becomes hidden.
    /// Uses view_id to make unregistration idempotent.
    pub fn register_view_closed(
        &mut self,
        window_id: WindowId,
        view_id: EntityId,
        _ctx: &mut ModelContext<Self>,
    ) {
        if let Some(views) = self.active_data_consumers_per_window.get_mut(&window_id) {
            views.remove(&view_id);
            if views.is_empty() {
                self.active_data_consumers_per_window.remove(&window_id);
            }
        }
    }

    /// Returns true if we have tasks or local conversations in this view
    pub fn has_items(&self) -> bool {
        !self.tasks.is_empty() || !self.conversations.is_empty()
    }

    /// Returns an iterator over all ambient agent tasks.
    pub fn tasks_iter(&self) -> impl Iterator<Item = &AmbientAgentTask> {
        self.tasks.values()
    }

    #[cfg(test)]
    pub(crate) fn insert_task_for_test(&mut self, task: AmbientAgentTask) {
        self.tasks.insert(task.task_id, task);
    }

    pub(crate) fn mark_task_execution_ended(
        &mut self,
        task_id: AmbientAgentTaskId,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(task) = self.tasks.get_mut(&task_id) else {
            return;
        };
        let was_active = task.has_active_execution();
        task.is_sandbox_running = false;
        if was_active {
            ctx.emit(AgentConversationsModelEvent::TasksUpdated);
        }
    }

    /// Returns normalized, owned entries for agent management/navigation surfaces.
    pub fn get_entries(
        &self,
        filters: &AgentManagementFilters,
        app: &AppContext,
    ) -> Vec<AgentConversationEntry> {
        let history_model = BlocklistAIHistoryModel::as_ref(app);
        let mut entries = Vec::new();
        let mut attached_conversation_ids = HashSet::new();
        let mut emitted_conversation_ids = HashSet::new();

        for task in self.tasks.values() {
            let entry = entry::entry_for_task(task, history_model, app);
            if let Some(conversation_id) = entry.identity.local_conversation_id {
                attached_conversation_ids.insert(conversation_id);
            }
            entries.push(entry);
        }

        for metadata in self.conversations.values() {
            let conversation_id = metadata.nav_data.id;
            if attached_conversation_ids.contains(&conversation_id) {
                continue;
            }
            let entry = entry::entry_for_conversation(metadata, history_model, app);
            emitted_conversation_ids.insert(conversation_id);
            entries.push(entry);
        }

        for metadata in history_model.get_local_conversations_metadata() {
            if attached_conversation_ids.contains(&metadata.id)
                || emitted_conversation_ids.contains(&metadata.id)
            {
                continue;
            }
            let nav_data =
                ConversationNavigationData::from_historical_conversation_metadata(metadata);
            entries.push(entry::entry_for_historical_metadata(
                metadata,
                nav_data,
                history_model,
                app,
            ));
        }

        entries
            .into_iter()
            .filter(|entry| entry.matches_filters(filters, app))
            .sorted_by(|a, b| b.display.last_updated.cmp(&a.display.last_updated))
            .collect()
    }

    pub fn get_entry_by_id(
        &self,
        id: &AgentConversationEntryId,
        app: &AppContext,
    ) -> Option<AgentConversationEntry> {
        let history_model = BlocklistAIHistoryModel::as_ref(app);
        match id {
            AgentConversationEntryId::AmbientRun(task_id) => self
                .tasks
                .get(task_id)
                .map(|task| entry::entry_for_task(task, history_model, app)),
            AgentConversationEntryId::Conversation(conversation_id) => self
                .conversations
                .get(conversation_id)
                .map(|metadata| entry::entry_for_conversation(metadata, history_model, app))
                .or_else(|| {
                    history_model
                        .get_conversation_metadata(conversation_id)
                        .map(|metadata| {
                            let nav_data =
                                ConversationNavigationData::from_historical_conversation_metadata(
                                    metadata,
                                );
                            entry::entry_for_historical_metadata(
                                metadata,
                                nav_data,
                                history_model,
                                app,
                            )
                        })
                }),
        }
    }

    pub fn resolve_open_action(
        subject: AgentConversationNavigationSubject,
        restore_layout: Option<RestoreConversationLayout>,
        app: &AppContext,
    ) -> Option<WorkspaceAction> {
        let model = Self::as_ref(app);
        match subject {
            AgentConversationNavigationSubject::Entry(id) => model
                .get_entry_by_id(&id, app)
                .and_then(|entry| model.resolve_entry_open_action(&entry, restore_layout, app)),
            AgentConversationNavigationSubject::ServerToken(server_token) => model
                .entry_for_server_token(&server_token, app)
                .and_then(|entry| model.resolve_entry_open_action(&entry, restore_layout, app))
                .or_else(|| {
                    Some(WorkspaceAction::OpenConversationTranscriptViewer {
                        ambient_agent_task_id: model.task_id_for_server_token(&server_token),
                        conversation_id: server_token,
                    })
                }),
        }
    }

    pub fn resolve_copy_link(
        subject: AgentConversationNavigationSubject,
        app: &AppContext,
    ) -> Option<String> {
        let model = Self::as_ref(app);
        match subject {
            AgentConversationNavigationSubject::Entry(id) => model
                .get_entry_by_id(&id, app)
                .and_then(|entry| model.resolve_entry_copy_link(&entry)),
            AgentConversationNavigationSubject::ServerToken(server_token) => model
                .entry_for_server_token(&server_token, app)
                .and_then(|entry| model.resolve_entry_copy_link(&entry))
                .or_else(|| Some(server_token.conversation_link())),
        }
    }

    fn resolve_entry_open_action(
        &self,
        entry: &AgentConversationEntry,
        restore_layout: Option<RestoreConversationLayout>,
        app: &AppContext,
    ) -> Option<WorkspaceAction> {
        // Zap: no `ActiveAgentViewsModel` (cloud-view state source, removed). There is no local
        // registry replacement for "is this ambient task's terminal view already open" (ambient
        // tasks are never populated outside tests — see `AgentConversationsModel::new`), so that
        // fast path is dropped here; it's not a functional loss because the
        // `OpenAmbientAgentSession` handler in `workspace/view.rs` already re-derives the open tab
        // via `find_tab_with_ambient_agent_conversation` before doing anything else.
        let history_model = BlocklistAIHistoryModel::as_ref(app);

        if let Some(conversation_id) = entry.identity.local_conversation_id {
            // Replaces the removed `ActiveAgentViewsModel::is_conversation_open` check: a
            // conversation counts as "open" when it still exists in the history model's memory.
            if history_model.conversation(&conversation_id).is_some() {
                if let Some(nav_data) = self
                    .conversations
                    .get(&conversation_id)
                    .map(|metadata| &metadata.nav_data)
                {
                    return Some(WorkspaceAction::RestoreOrNavigateToConversation {
                        conversation_id,
                        window_id: nav_data.window_id,
                        pane_view_locator: nav_data.pane_view_locator,
                        terminal_view_id: nav_data.terminal_view_id,
                        restore_layout,
                    });
                }

                if let Some(terminal_view_id) =
                    history_model.terminal_view_id_for_conversation(&conversation_id)
                {
                    return Some(WorkspaceAction::FocusTerminalViewInWorkspace {
                        terminal_view_id,
                    });
                }
            }
        }

        if let Some(task_id) = entry.identity.ambient_agent_task_id {
            let has_active_session = self
                .tasks
                .get(&task_id)
                .and_then(AmbientAgentTask::active_execution_session_id)
                .and_then(entry::parse_session_id)
                .is_some();
            if has_active_session {
                return Some(WorkspaceAction::OpenAmbientAgentSession { task_id });
            }
        }

        if let Some(conversation_id) = entry.identity.local_conversation_id {
            let nav_data = self
                .conversations
                .get(&conversation_id)
                .map(|metadata| &metadata.nav_data);
            if !entry.backing.has_cloud_data
                || entry.backing.has_local_persisted_data
                || entry.backing.has_loaded_conversation
                || nav_data.is_some()
            {
                return Some(WorkspaceAction::RestoreOrNavigateToConversation {
                    conversation_id,
                    window_id: nav_data.and_then(|nav_data| nav_data.window_id),
                    pane_view_locator: None,
                    terminal_view_id: nav_data.and_then(|nav_data| nav_data.terminal_view_id),
                    restore_layout,
                });
            }
        }

        entry
            .identity
            .server_conversation_token
            .as_ref()
            .map(|token| WorkspaceAction::OpenConversationTranscriptViewer {
                conversation_id: token.clone(),
                ambient_agent_task_id: entry.identity.ambient_agent_task_id,
            })
    }

    fn resolve_entry_copy_link(&self, entry: &AgentConversationEntry) -> Option<String> {
        if let Some(task_id) = entry.identity.ambient_agent_task_id {
            if let Some(session_link) = self.tasks.get(&task_id).and_then(|task| {
                task.has_active_execution()
                    .then(|| {
                        task.active_run_execution()
                            .session_link
                            .map(ToString::to_string)
                    })
                    .flatten()
            }) {
                return Some(session_link);
            }
        }

        entry
            .identity
            .server_conversation_token
            .as_ref()
            .map(ServerConversationToken::conversation_link)
    }

    fn entry_for_server_token(
        &self,
        server_token: &ServerConversationToken,
        app: &AppContext,
    ) -> Option<AgentConversationEntry> {
        let history_model = BlocklistAIHistoryModel::as_ref(app);
        if let Some(task) = self.tasks.values().find(|task| {
            task.conversation_id()
                .is_some_and(|conversation_id| conversation_id == server_token.as_str())
        }) {
            return Some(entry::entry_for_task(task, history_model, app));
        }

        let conversation_id = history_model.find_conversation_id_by_server_token(server_token)?;
        if let Some(task) = self.tasks.values().find(|task| {
            entry::conversation_id_shadowed_by_task(task, history_model) == Some(conversation_id)
        }) {
            return Some(entry::entry_for_task(task, history_model, app));
        }

        self.get_entry_by_id(
            &AgentConversationEntryId::Conversation(conversation_id),
            app,
        )
    }

    fn task_id_for_server_token(
        &self,
        server_token: &ServerConversationToken,
    ) -> Option<AmbientAgentTaskId> {
        self.tasks.values().find_map(|task| {
            task.conversation_id()
                .is_some_and(|conversation_id| conversation_id == server_token.as_str())
                .then_some(task.task_id)
        })
    }

    fn handle_history_event(
        &mut self,
        event: &BlocklistAIHistoryEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        if !FeatureFlag::InteractiveConversationManagementView.is_enabled() {
            return;
        }
        match event {
            // Events that affect conversation navigation data - need full sync
            BlocklistAIHistoryEvent::StartedNewConversation { .. }
            | BlocklistAIHistoryEvent::SetActiveConversation { .. }
            | BlocklistAIHistoryEvent::AppendedExchange { .. }
            | BlocklistAIHistoryEvent::SplitConversation { .. }
            | BlocklistAIHistoryEvent::RestoredConversations { .. }
            | BlocklistAIHistoryEvent::RemoveConversation { .. }
            | BlocklistAIHistoryEvent::DeletedConversation { .. }
            | BlocklistAIHistoryEvent::ClearedConversationsInTerminalView { .. }
            | BlocklistAIHistoryEvent::ClearedActiveConversation { .. } => {
                self.sync_conversations(ctx);
            }

            // Status changes - just trigger re-render since status is looked up at render time
            BlocklistAIHistoryEvent::UpdatedConversationStatus {
                update, new_status, ..
            } => {
                let kind = match update {
                    ConversationStatusUpdate::Restored => ConversationUpdateKind::Restored,
                    ConversationStatusUpdate::Changed { prev_status } => {
                        ConversationUpdateKind::StatusSet {
                            prev_filter: AgentRunDisplayStatus::from_conversation_status(
                                prev_status,
                            )
                            .status_filter(),
                            new_filter: AgentRunDisplayStatus::from_conversation_status(new_status)
                                .status_filter(),
                        }
                    }
                };
                ctx.emit(AgentConversationsModelEvent::ConversationUpdated { kind });
            }

            // Artifact changes - sync live artifacts into the cached task and notify.
            BlocklistAIHistoryEvent::UpdatedConversationArtifacts {
                conversation_id, ..
            } => {
                let conversation = BlocklistAIHistoryModel::as_ref(ctx).conversation(conversation_id);
                let Some(conversation) = conversation else {
                    return;
                };

                let task_id = conversation.task_id();
                if let Some(task_id) = task_id {
                    // If the conversation is associated with a task, update the saved task
                    // with live artifacts.
                    if let Some(task) = self.tasks.get_mut(&task_id) {
                        task.artifacts = conversation.artifacts().to_vec();
                        ctx.emit(AgentConversationsModelEvent::TasksUpdated);
                    }
                }
                ctx.emit(AgentConversationsModelEvent::ConversationArtifactsUpdated {
                    conversation_id: *conversation_id,
                });
            }

            // Task/exchange-level changes that don't affect conversation navigation.
            BlocklistAIHistoryEvent::CreatedSubtask { .. }
            | BlocklistAIHistoryEvent::UpgradedTask { .. }
            | BlocklistAIHistoryEvent::ReassignedExchange { .. }
            | BlocklistAIHistoryEvent::UpdatedTodoList { .. }
            | BlocklistAIHistoryEvent::UpdatedAutoexecuteOverride { .. }
            | BlocklistAIHistoryEvent::UpdatedConversationMetadata { .. }
            // UpdatedStreamingExchange covers streaming and other exchange-level updates but
            // doesn't change any ConversationNavigationData fields (title comes from
            // UpdateTaskDescription, last_updated uses exchange.start_time which is set at append time).
            | BlocklistAIHistoryEvent::UpdatedStreamingExchange { .. }
            | BlocklistAIHistoryEvent::ConversationOwnershipTransferred { .. }
            | BlocklistAIHistoryEvent::OrchestrationConfigUpdated { .. }
            | BlocklistAIHistoryEvent::ConversationUsageMetadataUpdated { .. }
            | BlocklistAIHistoryEvent::LocalSharedSessionEstablished { .. } => {}

            // A server/agent id was assigned to the conversation (e.g. via
            // StreamInit). Copy-link resolution depends on it, so notify
            // consumers that conversation capabilities changed.
            BlocklistAIHistoryEvent::ConversationAgentIdAssigned { .. } => {
                ctx.emit(AgentConversationsModelEvent::ConversationUpdated {
                    kind: ConversationUpdateKind::MetadataChanged,
                });
            }
        }
    }

    /// Get a task by its task ID
    pub fn get_task(&self, task_id: &AmbientAgentTaskId) -> Option<ConversationOrTask<'_>> {
        self.tasks.get(task_id).map(ConversationOrTask::Task)
    }

    /// Get raw task data by task ID
    pub fn get_task_data(&self, task_id: &AmbientAgentTaskId) -> Option<AmbientAgentTask> {
        self.tasks.get(task_id).cloned()
    }

    /// Reads locally cached task data by task ID.
    ///
    /// Zap no longer back-fills ambient agent tasks from the cloud. If the caller restored an old layout but the local model has no
    /// matching task, this returns `None`, handled by the existing panel degradation path.
    pub fn get_or_async_fetch_task_data(
        &self,
        task_id: &AmbientAgentTaskId,
    ) -> Option<AmbientAgentTask> {
        self.tasks.get(task_id).cloned()
    }

    /// Get a conversation by its AIConversationId
    pub fn get_conversation(
        &self,
        conversation_id: &AIConversationId,
    ) -> Option<ConversationOrTask<'_>> {
        self.conversations
            .get(conversation_id)
            .map(ConversationOrTask::Conversation)
    }

    /// Returns all (name, uid) pairs for creators of tasks in the model.
    ///
    /// We use this function to populate the available creator filter list
    /// based on the tasks we have.
    pub fn get_all_creators(&self, app: &AppContext) -> Vec<(String, String)> {
        let mut creators: Vec<(String, String)> = self
            .tasks
            .values()
            .filter_map(|task| {
                let name = entry::task_creator_name(task, app)?;
                let uid = entry::task_creator_uid(task)?;
                Some((name, uid))
            })
            .collect();

        // Include the current user since they may have local conversations
        let auth_state = AuthStateProvider::as_ref(app).get();
        if let (Some(name), Some(uid)) = (auth_state.display_name(), auth_state.user_id()) {
            creators.push((name, uid.to_string()));
        }

        creators.sort_by(|a, b| a.0.cmp(&b.0));
        creators.dedup_by(|a, b| a.0 == b.0);

        creators
    }

    pub fn mark_task_as_manually_opened(
        &mut self,
        task_id: AmbientAgentTaskId,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.manually_opened_task_ids.insert(task_id) {
            ctx.emit(AgentConversationsModelEvent::TaskManuallyOpened);
        }
    }

    pub fn is_task_manually_opened(&self, task_id: &AmbientAgentTaskId) -> bool {
        self.manually_opened_task_ids.contains(task_id)
    }

    /// Clears all stored conversation and task data in memory.
    /// This is used when logging out to ensure no conversation history persists across users.
    pub(crate) fn reset(&mut self) {
        self.tasks.clear();
        self.conversations.clear();
        self.active_data_consumers_per_window.clear();
        self.manually_opened_task_ids.clear();
        // Reset the initial load flag so that we can retry the initial sync with the new logged in user
        self.has_finished_initial_load = false;
    }
}

#[cfg(test)]
#[path = "agent_conversations_model_tests.rs"]
mod tests;
