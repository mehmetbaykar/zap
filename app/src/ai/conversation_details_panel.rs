//! A reusable side panel component for displaying conversation metadata.
//!
//! Zap adaptations from upstream (see the port report for the full rationale):
//! - No `TaskFetchError`/cloud fetch-error UI: this fork never fetches ambient-agent tasks over
//!   the network, so there is no fetch-error state to render. `PanelMode::Task::error_message`
//!   covers the one remaining local case (a restored layout references a task no longer held in
//!   memory) via `from_task_id`.
//! - No cloud-environment section (name / docker image / setup commands): `crate::ai::cloud_environments`
//!   does not exist in this fork. The environment ID is still shown (copyable) when present.
//! - No "Open in Oz" links anywhere (status chip, executor, skill section): the required
//!   `ChannelState::oz_root_url()` does not exist in this fork; there is no Oz web app to link to.
//! - No `HarnessAvailabilityModel`: the harness row is always shown (there is no fork equivalent
//!   of upstream's "hide the harness section" policy), using `crate::ai::harness_display` for the
//!   label/icon/color.
//! - "Continue locally" for third-party-harness ambient tasks is dropped:
//!   `WorkspaceAction::ContinueThirdPartyConversationLocally` does not exist in this fork (only
//!   `ContinueConversationLocally` for local interactive conversations does).
//! - Plan-artifact clicks dispatch `WorkspaceAction::OpenAIDocumentPane` directly (matching
//!   `crate::notifications::item_rendering`'s established pattern) instead of bubbling a
//!   `NotebookId`-based event, since the fork's `ArtifactButtonsRowEvent::OpenPlan` now carries a
//!   local `AIDocumentId`, not a cloud notebook id.

use std::str::FromStr;

use chrono::{DateTime, Duration, Local};
use warp_cli::agent::Harness;
use warp_cli::skill::SkillSpec;
use warp_core::ui::theme::color::internal_colors;
use warpui::clipboard::ClipboardContent;
use warpui::elements::{
    resizable_state_handle, Align, Border, ChildView, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, Element, Flex, MainAxisAlignment, MainAxisSize, MouseStateHandle,
    ParentElement, Radius, Resizable, ResizableStateHandle, Shrinkable, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element as _, Entity, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::ai::agent::conversation::{AIConversation, AIConversationId, ConversationStatus};
use crate::ai::agent_conversations_model::entry::PrincipalType;
use crate::ai::agent_conversations_model::{AgentConversationEntry, AgentRunDisplayStatus};
use crate::ai::agent_management::details_action_buttons::{
    ActionButtonsConfig, AgentDetailsButtonEvent, ConversationActionButtonsRow,
};
use crate::ai::agent_management::telemetry::{AgentManagementTelemetryEvent, OpenedFrom};
use crate::ai::ambient_agents::task::TaskPrincipalInfo;
use crate::ai::ambient_agents::{AmbientAgentTask, AmbientAgentTaskId};
use crate::ai::artifacts::{Artifact, ArtifactButtonsRow, ArtifactButtonsRowEvent};
use crate::ai::blocklist::BlocklistAIHistoryModel;
use crate::ai::document::ai_document_model::AIDocumentModel;
use crate::ai::harness_display;
use crate::appearance::Appearance;
use crate::send_telemetry_from_ctx;
use crate::ui_components::icons::Icon;
use crate::util::bindings::CustomAction;
use crate::util::time_format::{format_approx_duration_from_now, human_readable_precise_duration};
use crate::view_components::action_button::{ActionButton, ButtonSize, SecondaryTheme};
use crate::view_components::copyable_text_field::COPY_FEEDBACK_DURATION;
use crate::view_components::DismissibleToast;
use crate::workspace::{ForkedConversationDestination, ToastStack, WorkspaceAction};

const FIELD_SPACING: f32 = 16.0;
const HEADER_SPACING: f32 = 12.0;
const STATUS_ICON_SIZE: f32 = 12.0;
const HARNESS_CIRCLE_SIZE: f32 = 16.0;
const HARNESS_ICON_IN_CIRCLE: f32 = 9.0;
const LABEL_VALUE_GAP: f32 = 4.0;
const SECTION_HEADER_GAP: f32 = 8.0;

/// Panel rendering mode.
#[derive(Debug, Clone, PartialEq)]
enum PanelMode {
    Conversation {
        /// Working directory where the conversation took place.
        directory: Option<String>,
        /// Unique identifier for the conversation (server token), if this conversation was ever
        /// synced to a server. Zap never syncs, so this is always `None` for locally-created
        /// conversations; it is kept so restored/legacy records with a token still display it.
        server_conversation_id: Option<String>,
        /// Internal conversation ID (for action buttons).
        ai_conversation_id: Option<AIConversationId>,
        /// Status of the conversation.
        status: Option<ConversationStatus>,
    },
    Task {
        /// Unique identifier for the task.
        task_id: Option<AmbientAgentTaskId>,
        /// Working directory from the linked conversation, if available.
        directory: Option<String>,
        /// User-visible status derived from task and conversation state.
        display_status: Option<AgentRunDisplayStatus>,
        /// Error message, if we have one (e.g. a restored layout referenced a task that is no
        /// longer present locally).
        error_message: Option<String>,
        /// Environment ID.
        environment_id: Option<String>,
        /// Server conversation ID (for copy link).
        conversation_id: Option<String>,
    },
}

impl Default for PanelMode {
    fn default() -> Self {
        PanelMode::Conversation {
            directory: None,
            server_conversation_id: None,
            ai_conversation_id: None,
            status: None,
        }
    }
}

/// Groups mouse state handles for the panel.
#[derive(Default)]
struct PanelMouseStates {
    close_button: MouseStateHandle,
    copy_directory: MouseStateHandle,
    copy_conversation_id: MouseStateHandle,
    copy_run_id: MouseStateHandle,
    copy_environment_id: MouseStateHandle,
    copy_error: MouseStateHandle,
}

/// Tracks which copy button action was last triggered (for checkmark feedback).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CopyButtonKind {
    Directory,
    ConversationId,
    RunId,
    EnvironmentId,
    Error,
}

/// Information about a principal involved in a conversation.
#[derive(Debug, Clone)]
struct PrincipalInfo {
    /// Display name of the principal (or fallback identifier).
    pub display_name: String,
    /// Whether this principal is a service account.
    pub is_service_account: bool,
}

impl PrincipalInfo {
    fn new(display_name: String) -> Self {
        Self {
            display_name,
            is_service_account: false,
        }
    }
}

impl From<&TaskPrincipalInfo> for PrincipalInfo {
    fn from(p: &TaskPrincipalInfo) -> Self {
        Self {
            display_name: p.display_name.clone().unwrap_or_else(|| p.uid.clone()),
            is_service_account: PrincipalType::parse(&p.creator_type)
                .is_some_and(|pt| pt.is_service_account()),
        }
    }
}

/// Data model for the conversation details panel.
/// Any field that is left as None will not be rendered.
#[derive(Debug, Clone, Default)]
pub struct ConversationDetailsData {
    mode: PanelMode,
    title: String,
    /// Information about the creator.
    creator: Option<PrincipalInfo>,
    /// Principal the run executed as.
    executor: Option<PrincipalInfo>,
    /// When the conversation was created.
    created_at: Option<DateTime<Local>>,
    /// Total credits spent on the conversation/task.
    credits: Option<f32>,
    /// Total duration of the conversation.
    run_time: Option<Duration>,
    /// Artifacts created during the conversation (plans, PRs, branches).
    artifacts: Vec<Artifact>,
    /// Action to dispatch when "Open" button is clicked.
    open_action: Option<WorkspaceAction>,
    /// Source prompt that initiated this conversation/task.
    source_prompt: Option<String>,
    /// Copy link URL. Zap has no Warp-hosted session/conversation links, so this is always
    /// `None` for locally-created data (see `AgentConversationsModel::resolve_copy_link`); kept
    /// on the data model so a future local sharing mechanism has somewhere to plug in.
    copy_link_url: Option<String>,
    /// Parsed skill spec referenced by the task configuration.
    skill_spec: Option<SkillSpec>,
    /// Execution harness for this conversation/task.
    harness: Option<Harness>,
}

impl ConversationDetailsData {
    /// Builds details data from a normalized `AgentConversationEntry` (the agent management
    /// dashboard's data source). `task`, when the entry is task-backed, supplies fields not
    /// carried on the entry itself (skill spec, precise run time).
    pub fn from_agent_conversation_entry(
        entry: &AgentConversationEntry,
        task: Option<&AmbientAgentTask>,
        open_action: Option<WorkspaceAction>,
        copy_link_url: Option<String>,
    ) -> Self {
        let creator = entry
            .display
            .creator
            .name
            .clone()
            .map(PrincipalInfo::new);
        let executor = entry.display.executor.as_ref().and_then(|e| {
            let display_name = e.name.clone().or_else(|| e.uid.clone())?;
            Some(PrincipalInfo {
                display_name,
                is_service_account: e.principal_type.is_some_and(|pt| pt.is_service_account()),
            })
        });
        let created_at = Some(entry.display.created_at.with_timezone(&Local));
        let source_prompt = entry.display.initial_query.clone();
        let harness = entry.display.harness;

        if let Some(task_id) = entry.identity.ambient_agent_task_id {
            let error_message = task.and_then(|task| {
                task.state
                    .is_failure_like()
                    .then(|| task.status_message.as_ref().map(|m| m.message.clone()))
                    .flatten()
            });
            // Fall back to the entry's denormalized total when the task record isn't
            // currently loaded, so the panel stays consistent with the card metadata
            // (which always reads `entry.display.request_usage`).
            let credits = task
                .and_then(AmbientAgentTask::credits_used)
                .or(entry.display.request_usage);
            let skill_spec = task
                .and_then(|task| task.agent_config_snapshot.as_ref())
                .and_then(|config| config.skill_spec.as_ref())
                .and_then(|spec_str| SkillSpec::from_str(spec_str).ok());

            return ConversationDetailsData {
                mode: PanelMode::Task {
                    task_id: Some(task_id),
                    directory: entry.display.working_directory.clone(),
                    display_status: Some(entry.display.status.clone()),
                    error_message,
                    environment_id: entry.display.environment_id.clone(),
                    conversation_id: entry
                        .identity
                        .server_conversation_token
                        .as_ref()
                        .map(|token| token.as_str().to_string()),
                },
                title: entry.display.title.clone(),
                creator,
                executor,
                created_at,
                credits,
                run_time: task.and_then(AmbientAgentTask::run_time),
                artifacts: entry.display.artifacts.clone(),
                open_action,
                source_prompt,
                copy_link_url,
                skill_spec,
                harness,
            };
        }

        ConversationDetailsData {
            mode: PanelMode::Conversation {
                directory: entry.display.working_directory.clone(),
                server_conversation_id: entry
                    .identity
                    .server_conversation_token
                    .as_ref()
                    .map(|token| token.as_str().to_string()),
                ai_conversation_id: entry.identity.local_conversation_id,
                status: Some(entry.display.status.to_conversation_status()),
            },
            title: entry.display.title.clone(),
            creator,
            executor: None,
            created_at,
            credits: entry.display.request_usage,
            run_time: None,
            artifacts: entry.display.artifacts.clone(),
            open_action,
            source_prompt,
            copy_link_url,
            skill_spec: None,
            harness,
        }
    }

    /// Builds details data directly from an `AmbientAgentTask` (used by the WASM transcript /
    /// shared-session details panel when the focused terminal view is an ambient-agent session).
    pub fn from_task(
        task: &AmbientAgentTask,
        open_action: Option<WorkspaceAction>,
        copy_link_url: Option<String>,
        app: &AppContext,
    ) -> Self {
        let display_status = AgentRunDisplayStatus::from_task(task, app);
        let error_message = task
            .state
            .is_failure_like()
            .then(|| task.status_message.as_ref().map(|m| m.message.clone()))
            .flatten();
        let harness = task.agent_config_snapshot.as_ref().and_then(|config| {
            config
                .harness
                .as_ref()
                .map(|h| h.harness_type)
                .or(Some(Harness::Oz))
        });
        let skill_spec = task
            .agent_config_snapshot
            .as_ref()
            .and_then(|config| config.skill_spec.as_ref())
            .and_then(|spec_str| SkillSpec::from_str(spec_str).ok());

        ConversationDetailsData {
            mode: PanelMode::Task {
                task_id: Some(task.task_id),
                directory: None,
                display_status: Some(display_status),
                error_message,
                environment_id: task
                    .agent_config_snapshot
                    .as_ref()
                    .and_then(|s| s.environment_id.clone()),
                conversation_id: task.conversation_id().map(str::to_string),
            },
            title: task.title.clone(),
            creator: task.creator.as_ref().map(PrincipalInfo::from),
            executor: task.executor.as_ref().map(PrincipalInfo::from),
            created_at: Some(task.created_at.with_timezone(&Local)),
            credits: task.credits_used(),
            run_time: task.run_time(),
            artifacts: task.artifacts.clone(),
            open_action,
            source_prompt: Some(task.prompt.clone()),
            copy_link_url,
            skill_spec,
            harness,
        }
    }

    /// Builds a minimal error-state panel for a task ID we have no local record of (e.g. a
    /// restored layout referenced an ambient-agent task that is no longer held in memory). Zap
    /// never fetches task data over the network, so there is no distinct "fetch failed" case:
    /// "not found locally" is the only error this constructor represents.
    pub fn from_task_id(task_id: AmbientAgentTaskId, error_message: Option<String>) -> Self {
        ConversationDetailsData {
            mode: PanelMode::Task {
                task_id: Some(task_id),
                directory: None,
                display_status: None,
                error_message,
                environment_id: None,
                conversation_id: None,
            },
            title: String::new(),
            ..Default::default()
        }
    }

    /// Builds details data from an in-memory `AIConversation` for a local (non-ambient)
    /// conversation. Used by the WASM transcript/shared-session details panel.
    ///
    /// Zap conversations have no server-side creator/executor metadata (there is no server); the
    /// creator/executor fields are always `None` here.
    pub fn from_conversation(conversation: &AIConversation, _app: &AppContext) -> Self {
        let source_prompt = conversation.latest_user_query();
        let title = conversation
            .title()
            .clone()
            .or_else(|| source_prompt.clone())
            .unwrap_or_default();

        ConversationDetailsData {
            mode: PanelMode::Conversation {
                directory: conversation.initial_working_directory(),
                server_conversation_id: conversation
                    .server_conversation_token()
                    .map(|token| token.as_str().to_string()),
                ai_conversation_id: None,
                status: Some(conversation.status().clone()),
            },
            title,
            creator: None,
            executor: None,
            created_at: None,
            credits: Some(conversation.credits_spent()),
            run_time: None,
            artifacts: conversation.artifacts().to_vec(),
            open_action: None,
            source_prompt,
            copy_link_url: None,
            skill_spec: None,
            harness: conversation.orchestration_harness().or(Some(Harness::Oz)),
        }
    }

    /// Builds details data for a local interactive conversation from its normalized display
    /// fields directly (the agent-management dashboard's data model already has these
    /// precomputed; this avoids re-deriving them from a live `AIConversation`).
    #[allow(clippy::too_many_arguments)]
    pub fn from_conversation_metadata(
        conversation_id: AIConversationId,
        title: String,
        directory: Option<String>,
        created_at: DateTime<Local>,
        server_conversation_id: Option<String>,
        status: Option<ConversationStatus>,
        creator_name: Option<String>,
        artifacts: Vec<Artifact>,
        open_action: Option<WorkspaceAction>,
        source_prompt: Option<String>,
        copy_link_url: Option<String>,
        credits: Option<f32>,
        harness: Option<Harness>,
    ) -> Self {
        ConversationDetailsData {
            mode: PanelMode::Conversation {
                directory,
                server_conversation_id,
                ai_conversation_id: Some(conversation_id),
                status,
            },
            title,
            creator: creator_name.map(PrincipalInfo::new),
            executor: None,
            created_at: Some(created_at),
            credits,
            run_time: None,
            artifacts,
            open_action,
            source_prompt,
            copy_link_url,
            skill_spec: None,
            harness,
        }
    }
}

/// Actions dispatched by button clicks / keybindings within the panel (internal).
#[derive(Debug, Clone)]
pub enum ConversationDetailsPanelAction {
    Close,
    CopyDirectory,
    CopyConversationId,
    CopyRunId,
    CopyEnvironmentId,
    CopyError,
}

/// Events emitted by the panel to its parent view.
#[derive(Debug)]
pub enum ConversationDetailsPanelEvent {
    Close,
}

/// Reusable side panel showing metadata for a conversation or ambient-agent task run.
pub struct ConversationDetailsPanel {
    /// Whether the action-buttons row (open / cancel / fork / copy link) is shown. The WASM
    /// transcript panel passes `false` (read-only); the agent management dashboard passes `true`.
    show_actions: bool,
    resizable_state: ResizableStateHandle,
    data: ConversationDetailsData,
    mouse_states: PanelMouseStates,
    last_copied: Option<CopyButtonKind>,
    action_buttons: ViewHandle<ConversationActionButtonsRow>,
    artifact_buttons: Option<ViewHandle<ArtifactButtonsRow>>,
}

pub fn init(_app: &mut AppContext) {}

impl ConversationDetailsPanel {
    pub fn new(show_actions: bool, width: f32, ctx: &mut ViewContext<Self>) -> Self {
        let action_buttons = ctx.add_typed_action_view(ConversationActionButtonsRow::new);
        ctx.subscribe_to_view(&action_buttons, |me, _, event, ctx| {
            me.handle_action_buttons_event(event, ctx);
        });

        Self {
            show_actions,
            resizable_state: resizable_state_handle(width),
            data: ConversationDetailsData::default(),
            mouse_states: PanelMouseStates::default(),
            last_copied: None,
            action_buttons,
            artifact_buttons: None,
        }
    }

    pub fn set_conversation_details(&mut self, data: ConversationDetailsData, ctx: &mut ViewContext<Self>) {
        self.last_copied = None;

        if self.show_actions {
            let config = match &data.mode {
                PanelMode::Task { task_id, .. } => ActionButtonsConfig {
                    open_action: data.open_action.clone(),
                    cancel_task_id: task_id
                        .filter(|_| {
                            data.display_status()
                                .map(|s| s.is_cancellable())
                                .unwrap_or(false)
                        }),
                    fork_conversation_id: None,
                    view_details_item_id: None,
                    copy_link_url: data.copy_link_url.clone(),
                },
                PanelMode::Conversation {
                    ai_conversation_id, ..
                } => ActionButtonsConfig {
                    open_action: data.open_action.clone(),
                    cancel_task_id: None,
                    fork_conversation_id: *ai_conversation_id,
                    view_details_item_id: None,
                    copy_link_url: data.copy_link_url.clone(),
                },
            };
            self.action_buttons
                .update(ctx, |row, ctx| row.set_config(config, ctx));
        }

        self.artifact_buttons = if data.artifacts.is_empty() {
            None
        } else {
            let view = ctx.add_typed_action_view(|ctx| ArtifactButtonsRow::new(&data.artifacts, ctx));
            ctx.subscribe_to_view(&view, |me, _, event, ctx| {
                me.handle_artifact_buttons_event(event, ctx);
            });
            Some(view)
        };

        self.data = data;
        ctx.notify();
    }

    fn handle_action_buttons_event(&mut self, event: &AgentDetailsButtonEvent, ctx: &mut ViewContext<Self>) {
        match event {
            AgentDetailsButtonEvent::Open => {
                if let Some(action) = self.data.open_action.clone() {
                    ctx.dispatch_typed_action(&action);
                }
            }
            AgentDetailsButtonEvent::CancelTask { task_id } => {
                send_telemetry_from_ctx!(
                    AgentManagementTelemetryEvent::CloudRunCancelled {
                        task_id: task_id.to_string(),
                    },
                    ctx
                );
                // Zap has no server to cancel an ambient-agent task on; ambient tasks are never
                // populated outside of tests, so this path is effectively unreachable in
                // practice. Surface a toast rather than silently no-op-ing in case it ever is.
                let window_id = ctx.window_id();
                ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                    let toast = DismissibleToast::default("Nothing to cancel".to_string());
                    toast_stack.add_ephemeral_toast(toast, window_id, ctx);
                });
            }
            AgentDetailsButtonEvent::ForkConversation { conversation_id } => {
                send_telemetry_from_ctx!(
                    AgentManagementTelemetryEvent::ConversationForked {
                        conversation_id: conversation_id.to_string(),
                    },
                    ctx
                );
                ctx.dispatch_typed_action(&WorkspaceAction::ForkAIConversation {
                    conversation_id: *conversation_id,
                    fork_from_exchange: None,
                    summarize_after_fork: false,
                    summarization_prompt: None,
                    initial_prompt: None,
                    initial_attachments: vec![],
                    destination: ForkedConversationDestination::NewTab,
                });
            }
            AgentDetailsButtonEvent::ViewDetails { .. } => {}
            AgentDetailsButtonEvent::CopyLink { link } => {
                ctx.clipboard()
                    .write(ClipboardContent::plain_text(link.clone()));
            }
        }
    }

    fn handle_artifact_buttons_event(&mut self, event: &ArtifactButtonsRowEvent, ctx: &mut ViewContext<Self>) {
        match event {
            ArtifactButtonsRowEvent::OpenPlan { document_uid } => {
                send_telemetry_from_ctx!(
                    AgentManagementTelemetryEvent::ArtifactClicked {
                        artifact_type: crate::ai::agent_management::telemetry::ArtifactType::Plan
                    },
                    ctx
                );
                let document_version = AIDocumentModel::as_ref(ctx)
                    .get_current_document(document_uid)
                    .map(|doc| doc.version)
                    .unwrap_or_default();
                ctx.dispatch_typed_action(&WorkspaceAction::OpenAIDocumentPane {
                    document_id: *document_uid,
                    document_version,
                });
            }
            ArtifactButtonsRowEvent::CopyBranch { branch } => {
                send_telemetry_from_ctx!(
                    AgentManagementTelemetryEvent::ArtifactClicked {
                        artifact_type: crate::ai::agent_management::telemetry::ArtifactType::Branch
                    },
                    ctx
                );
                ctx.clipboard()
                    .write(ClipboardContent::plain_text(branch.clone()));
                let window_id = ctx.window_id();
                ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                    let toast = DismissibleToast::default("Copied branch name".to_string());
                    toast_stack.add_ephemeral_toast(toast, window_id, ctx);
                });
            }
            ArtifactButtonsRowEvent::OpenPullRequest { url } => {
                send_telemetry_from_ctx!(
                    AgentManagementTelemetryEvent::ArtifactClicked {
                        artifact_type: crate::ai::agent_management::telemetry::ArtifactType::PullRequest
                    },
                    ctx
                );
                ctx.open_url(url);
            }
        }
    }

    fn render_close_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        appearance
            .ui_builder()
            .close_button(16., self.mouse_states.close_button.clone())
            .build()
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(ConversationDetailsPanelAction::Close);
            })
            .finish()
    }

    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let title = Text::new(
            self.data.title.clone(),
            appearance.ui_font_family(),
            appearance.ui_font_size() + 2.,
        )
        .with_style(Properties::default().weight(Weight::Semibold))
        .with_color(theme.active_ui_text_color().into())
        .soft_wrap(true)
        .finish();

        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(HEADER_SPACING)
            .with_child(Shrinkable::new(1., title).finish())
            .with_child(self.render_close_button(appearance))
            .finish()
    }

    fn render_status_row(&self, appearance: &Appearance) -> Option<Box<dyn Element>> {
        let theme = appearance.theme();
        let status = self.data.display_status()?;
        let (icon, color) = status.status_icon_and_color(theme);
        let icon_element = ConstrainedBox::new(icon.to_warpui_icon(color.into()).finish())
            .with_width(STATUS_ICON_SIZE)
            .with_height(STATUS_ICON_SIZE)
            .finish();
        let text = Text::new_inline(
            status.to_string(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(theme.active_ui_text_color().into())
        .finish();

        Some(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(LABEL_VALUE_GAP)
                .with_child(icon_element)
                .with_child(text)
                .finish(),
        )
    }

    fn render_field_row(&self, label: &str, value: String, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let label_text = Text::new_inline(
            label.to_string(),
            appearance.ui_font_family(),
            appearance.ui_font_size() - 1.,
        )
        .with_color(theme.nonactive_ui_text_color().into())
        .finish();
        let value_text = Text::new_inline(value, appearance.ui_font_family(), appearance.ui_font_size())
            .with_color(theme.active_ui_text_color().into())
            .soft_wrap(true)
            .finish();

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(LABEL_VALUE_GAP)
            .with_child(label_text)
            .with_child(value_text)
            .finish()
    }

    /// Renders a label/value row where the value can be clicked to copy it, showing a brief
    /// "Copied" confirmation in place of the value on click (feedback state tracked via
    /// `self.last_copied`, mirroring the pattern in `details_action_buttons.rs`'s copy-link
    /// button rather than the standalone `copyable_text_field` component, which is shaped for a
    /// single self-contained field rather than a label+value row).
    fn render_copyable_field_row(
        &self,
        label: &str,
        value: &str,
        kind: CopyButtonKind,
        mouse_state: MouseStateHandle,
        action: ConversationDetailsPanelAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let is_copied = self.last_copied == Some(kind);
        let display_text = if is_copied {
            "Copied".to_string()
        } else {
            value.to_string()
        };
        let font_family = appearance.ui_font_family();
        let font_size = appearance.ui_font_size();
        let text_color = if is_copied {
            theme.ansi_fg_green()
        } else {
            internal_colors::text_main(theme, theme.surface_2())
        };

        let clickable_value = warpui::elements::Hoverable::new(mouse_state, move |_state| {
            Text::new_inline(display_text.clone(), font_family, font_size)
                .with_color(text_color)
                .soft_wrap(true)
                .finish()
        })
        .with_cursor(warpui::platform::Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(action.clone());
        })
        .finish();

        let label_text = Text::new_inline(
            label.to_string(),
            appearance.ui_font_family(),
            appearance.ui_font_size() - 1.,
        )
        .with_color(theme.nonactive_ui_text_color().into())
        .finish();

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(LABEL_VALUE_GAP)
            .with_child(label_text)
            .with_child(clickable_value)
            .finish()
    }

    fn render_harness_row(&self, appearance: &Appearance) -> Option<Box<dyn Element>> {
        let harness = self.data.harness?;
        let theme = appearance.theme();

        let circle = ConstrainedBox::new(
            Container::new(
                Align::new(
                    ConstrainedBox::new(
                        harness_display::icon_for(harness)
                            .to_warpui_icon(harness_display::icon_fill_on_circle(harness, theme))
                            .finish(),
                    )
                    .with_width(HARNESS_ICON_IN_CIRCLE)
                    .with_height(HARNESS_ICON_IN_CIRCLE)
                    .finish(),
                )
                .finish(),
            )
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
            .with_background(harness_display::circle_background(harness, theme))
            .finish(),
        )
        .with_width(HARNESS_CIRCLE_SIZE)
        .with_height(HARNESS_CIRCLE_SIZE)
        .finish();

        let label = Text::new_inline(
            harness_display::display_name(harness),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(theme.active_ui_text_color().into())
        .finish();

        let label_text = Text::new_inline(
            "Harness",
            appearance.ui_font_family(),
            appearance.ui_font_size() - 1.,
        )
        .with_color(theme.nonactive_ui_text_color().into())
        .finish();

        Some(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(LABEL_VALUE_GAP)
                .with_child(label_text)
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(LABEL_VALUE_GAP)
                        .with_child(circle)
                        .with_child(label)
                        .finish(),
                )
                .finish(),
        )
    }

    fn render_error_banner(&self, message: &str, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text = Text::new(
            message.to_string(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(theme.ansi_fg_red().into())
        .soft_wrap(true)
        .finish();

        Container::new(text)
            .with_uniform_padding(8.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .with_border(Border::all(1.).with_border_fill(theme.ansi_fg_red()))
            .finish()
    }

    fn render_fields(&self, appearance: &Appearance) -> Vec<Box<dyn Element>> {
        let mut fields: Vec<Box<dyn Element>> = Vec::new();

        if let Some(status_row) = self.render_status_row(appearance) {
            fields.push(status_row);
        }

        match &self.data.mode {
            PanelMode::Task { error_message, .. } => {
                if let Some(message) = error_message {
                    fields.push(self.render_error_banner(message, appearance));
                }
            }
            PanelMode::Conversation { .. } => {}
        }

        if let Some(creator) = &self.data.creator {
            fields.push(self.render_field_row("Creator", creator.display_name.clone(), appearance));
        }
        if let Some(executor) = &self.data.executor {
            fields.push(self.render_field_row("Executor", executor.display_name.clone(), appearance));
        }
        if let Some(created_at) = self.data.created_at {
            fields.push(self.render_field_row(
                "Created",
                format_approx_duration_from_now(created_at),
                appearance,
            ));
        }
        if let Some(run_time) = self.data.run_time {
            fields.push(self.render_field_row(
                "Run time",
                human_readable_precise_duration(run_time),
                appearance,
            ));
        }
        if let Some(credits) = self.data.credits {
            fields.push(self.render_field_row("Credits used", format!("{credits:.2}"), appearance));
        }

        let directory = self.data.directory();
        if let Some(directory) = directory {
            fields.push(self.render_copyable_field_row(
                "Directory",
                &directory,
                CopyButtonKind::Directory,
                self.mouse_states.copy_directory.clone(),
                ConversationDetailsPanelAction::CopyDirectory,
                appearance,
            ));
        }

        if let PanelMode::Task {
            task_id: Some(task_id),
            ..
        } = &self.data.mode
        {
            fields.push(self.render_copyable_field_row(
                "Run ID",
                &task_id.to_string(),
                CopyButtonKind::RunId,
                self.mouse_states.copy_run_id.clone(),
                ConversationDetailsPanelAction::CopyRunId,
                appearance,
            ));
        }

        if let PanelMode::Task {
            environment_id: Some(environment_id),
            ..
        } = &self.data.mode
        {
            fields.push(self.render_copyable_field_row(
                "Environment ID",
                environment_id,
                CopyButtonKind::EnvironmentId,
                self.mouse_states.copy_environment_id.clone(),
                ConversationDetailsPanelAction::CopyEnvironmentId,
                appearance,
            ));
        }

        if let Some(harness_row) = self.render_harness_row(appearance) {
            fields.push(harness_row);
        }

        if let Some(skill_spec) = &self.data.skill_spec {
            fields.push(self.render_field_row("Skill", skill_spec.to_string(), appearance));
        }

        if let Some(source_prompt) = &self.data.source_prompt {
            fields.push(self.render_field_row("Prompt", source_prompt.clone(), appearance));
        }

        fields
    }
}

impl ConversationDetailsData {
    fn display_status(&self) -> Option<AgentRunDisplayStatus> {
        match &self.mode {
            PanelMode::Task { display_status, .. } => display_status.clone(),
            PanelMode::Conversation { status, .. } => status
                .as_ref()
                .map(AgentRunDisplayStatus::from_conversation_status),
        }
    }

    fn directory(&self) -> Option<String> {
        match &self.mode {
            PanelMode::Task { directory, .. } | PanelMode::Conversation { directory, .. } => {
                directory.clone()
            }
        }
    }
}

impl Entity for ConversationDetailsPanel {
    type Event = ConversationDetailsPanelEvent;
}

impl TypedActionView for ConversationDetailsPanel {
    type Action = ConversationDetailsPanelAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            ConversationDetailsPanelAction::Close => {
                ctx.emit(ConversationDetailsPanelEvent::Close);
            }
            ConversationDetailsPanelAction::CopyDirectory => {
                if let Some(directory) = self.data.directory() {
                    ctx.clipboard().write(ClipboardContent::plain_text(directory));
                    self.mark_copied(CopyButtonKind::Directory, ctx);
                }
            }
            ConversationDetailsPanelAction::CopyConversationId => {
                let id = match &self.data.mode {
                    PanelMode::Conversation {
                        server_conversation_id,
                        ..
                    } => server_conversation_id.clone(),
                    PanelMode::Task { conversation_id, .. } => conversation_id.clone(),
                };
                if let Some(id) = id {
                    ctx.clipboard().write(ClipboardContent::plain_text(id));
                    self.mark_copied(CopyButtonKind::ConversationId, ctx);
                }
            }
            ConversationDetailsPanelAction::CopyRunId => {
                if let PanelMode::Task {
                    task_id: Some(task_id),
                    ..
                } = &self.data.mode
                {
                    ctx.clipboard()
                        .write(ClipboardContent::plain_text(task_id.to_string()));
                    self.mark_copied(CopyButtonKind::RunId, ctx);
                }
            }
            ConversationDetailsPanelAction::CopyEnvironmentId => {
                if let PanelMode::Task {
                    environment_id: Some(environment_id),
                    ..
                } = &self.data.mode
                {
                    ctx.clipboard()
                        .write(ClipboardContent::plain_text(environment_id.clone()));
                    self.mark_copied(CopyButtonKind::EnvironmentId, ctx);
                }
            }
            ConversationDetailsPanelAction::CopyError => {
                if let PanelMode::Task {
                    error_message: Some(message),
                    ..
                } = &self.data.mode
                {
                    ctx.clipboard()
                        .write(ClipboardContent::plain_text(message.clone()));
                    self.mark_copied(CopyButtonKind::Error, ctx);
                }
            }
        }
    }
}

impl ConversationDetailsPanel {
    fn mark_copied(&mut self, kind: CopyButtonKind, ctx: &mut ViewContext<Self>) {
        self.last_copied = Some(kind);
        ctx.notify();
        let duration = COPY_FEEDBACK_DURATION;
        ctx.spawn(
            async move {
                warpui::r#async::Timer::after(duration).await;
            },
            move |me, _, ctx| {
                if me.last_copied == Some(kind) {
                    me.last_copied = None;
                    ctx.notify();
                }
            },
        );
    }
}

impl View for ConversationDetailsPanel {
    fn ui_name() -> &'static str {
        "ConversationDetailsPanel"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let mut content = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(FIELD_SPACING)
            .with_child(self.render_header(appearance));

        for field in self.render_fields(appearance) {
            content.add_child(field);
        }

        if let Some(artifact_buttons) = &self.artifact_buttons {
            content.add_child(
                Container::new(ChildView::new(artifact_buttons).finish())
                    .with_margin_top(SECTION_HEADER_GAP)
                    .finish(),
            );
        }

        if self.show_actions {
            content.add_child(
                Container::new(ChildView::new(&self.action_buttons).finish())
                    .with_margin_top(SECTION_HEADER_GAP)
                    .finish(),
            );
        }

        let scrollable = Container::new(content.finish())
            .with_uniform_padding(16.)
            .finish();

        let panel = Container::new(scrollable)
            .with_background(theme.surface_1())
            .with_border(Border::left(1.).with_border_fill(theme.outline()))
            .finish();

        // This panel always sits at the right edge of its container (the agent management view's
        // details panel, and the WASM transcript panel), so its drag handle is on the left.
        Resizable::new(self.resizable_state.clone(), panel)
            .with_dragbar_side(warpui::elements::DragBarSide::Left)
            .finish()
    }
}

#[cfg(test)]
#[path = "conversation_details_panel_tests.rs"]
mod tests;
