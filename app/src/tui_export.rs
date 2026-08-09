//! Public app APIs used by the `warp_tui` frontend.

pub use repo_metadata::repositories::RepoDetectionSource;
pub use warp_cli::agent::Harness;
use warpui::SingletonEntity as _;

pub use self::history::{TuiUpArrowHistoryItem, TuiUpArrowHistoryItemKind, tui_up_arrow_history};
pub use crate::ai::agent::api::ServerConversationToken;
pub use crate::ai::agent::conversation::{
    AIConversation, AIConversationAutoexecuteMode, AIConversationId, ConversationStatus, TodoStatus,
};
pub use crate::ai::agent::task::TaskId;
pub use crate::ai::agent::todos::AIAgentTodoList;
pub use crate::ai::agent::{
    AIAgentAction, AIAgentActionId, AIAgentActionResult, AIAgentActionResultType,
    AIAgentActionType, AIAgentContext, AIAgentExchangeId, AIAgentInput, AIAgentOutput,
    AIAgentOutputMessage, AIAgentOutputMessageType, AIAgentPtyWriteMode, AIAgentText,
    AIAgentTextSection, AIAgentTodo, AIAgentTodoId, AskUserQuestionResult, CancellationReason,
    FileGlobV2Result, GrepResult, MessageId, RequestCommandOutputResult, RunAgentsAgentOutcomeKind,
    RunAgentsResult, SearchCodebaseFailureReason, SearchCodebaseResult, ServerOutputId, Shared,
    ShellCommandDelay, StartAgentExecutionMode, SuggestNewConversationResult, SummarizationType,
    TodoOperation, UserQueryMode,
};
pub use crate::ai::agent_conversations_model::{
    AgentConversationEntry, AgentConversationEntryId, AgentConversationListEntryState,
    AgentConversationListPolicy, AgentConversationsModel, AgentConversationsModelEvent,
    AgentManagementFilters, AgentRunDisplayStatus, HarnessFilter, OwnerFilter,
    query_conversation_entries,
};
pub use crate::ai::blocklist::agent_view::{
    AgentViewController, AgentViewDisplayMode, AgentViewEntryOrigin, EnterAgentViewError,
    EphemeralMessageModel,
};
pub use crate::ai::blocklist::block::cli_controller::{
    CLISubagentController, CLISubagentEvent, CLISubagentTarget, LongRunningCommandControlState,
    UserTakeOverReason,
};
pub use crate::ai::blocklist::block::model::{
    AIBlockModel, AIBlockModelImpl, AIBlockOutputStatus, AIRequestType, OutputStatusUpdateCallback,
};
pub use crate::ai::blocklist::conversation_selection::{
    ConversationSelection, ConversationSelectionEvent, ConversationSelectionHandle,
    PendingQueryState,
};
pub use crate::ai::blocklist::diff_storage::{
    DiffStorage, DiffStorageHelper, FileSnapshot, RegisteredDiffStorage, SaveFuture,
    UpdatedFileState,
};
pub use crate::ai::blocklist::diff_types::{DiffSessionType, FileDiff, changed_lines_from_op};
pub use crate::ai::blocklist::history_model::{
    BlocklistAIHistoryEvent, BlocklistAIHistoryModel, ConversationStatusUpdate,
    LoadedConversationData,
};
pub use crate::ai::blocklist::view_util::format_credits;
pub use crate::ai::blocklist::{
    AIActionStatus, BlocklistAIActionEvent, BlocklistAIActionModel, BlocklistAIContextModel,
    BlocklistAIController, BlocklistAIInputModel, InputConfig, InputModePolicy,
    InputModePolicyHandle, InputType, InputTypeAutoDetectionSource, PolicyConfigUpdate,
    RequestFileEditsExecutor, ShellCommandExecutor, ShellCommandExecutorEvent,
    block_context_from_terminal_model,
};
#[cfg(feature = "local_fs")]
pub use crate::ai::conversation_export::{
    ConversationFileExport, ConversationFileExportError, export_conversation_markdown,
};
pub use crate::ai::llms::{LLMId, LLMInfo, LLMPreferences, LLMPreferencesEvent};
pub use crate::ai::skills::{SkillManager, SkillReference};
pub use crate::appearance::Appearance;
pub use crate::banner::BannerState;
pub use crate::changelog_model::{
    ChangelogModel, ChangelogRequestType, ChangelogState, Event as ChangelogModelEvent,
};
pub use crate::code::DiffResult;
pub use crate::code_review::git_repo_model::{
    GitRepoModels, GitRepoStatusModel, GitStatusMetadata,
};
pub use crate::search::slash_command_menu::static_commands::commands::{
    self as slash_commands, COMMAND_REGISTRY,
};
pub use crate::search::slash_command_menu::{SlashCommandId, StaticCommand};
pub use crate::settings::AISettingsChangedEvent;
pub use crate::terminal::color::{Colors as TerminalColors, List as TerminalColorList};
pub use crate::terminal::conversation_restoration::{
    ConversationBlockRestorationPlan, RestoredConversationExchange,
    prepare_conversation_block_restoration,
};
pub use crate::terminal::event::AfterBlockCompletedEvent;
pub use crate::terminal::input::CommandExecutionSource;
pub use crate::terminal::input::models::{ModelPickerChoice, query_model_picker_choices};
pub use crate::terminal::input::skills::{
    AcceptSkill, LOCAL_SKILLS_REMOTE_EXECUTION_ERROR_MESSAGE, SelectableSkill,
    query_selectable_skills,
};
pub use crate::terminal::input::slash_command_model::{
    DetectedCommand, DetectedSkillCommand, ParsedSlashCommandInput,
    slash_command_composition_filter,
};
pub use crate::terminal::input::slash_commands::{
    AcceptSlashCommandOrSavedPrompt, InlineItem, SlashCommandDataSource, SlashCommandMixer,
    SlashCommandSelectionBehavior, TuiDataSourceArgs as TuiSlashCommandDataSourceArgs,
    TuiSlashCommand, TuiSlashCommandDataSource, TuiZeroStateDataSource, UpdatedActiveCommands,
    build_slash_command_mixer, record_saved_prompt_accepted, record_static_slash_command_accepted,
    saved_prompt_text_for_id, should_close_slash_command_menu_for_exact_match,
    slash_command_is_submitted_as_prompt, slash_command_is_supported_in_tui, slash_command_query,
    slash_command_selection_behavior,
};
pub use crate::terminal::local_tty::{
    TerminalManager as LocalTtyTerminalManager, TerminalManagerInit, TerminalSurfaceInit,
    TerminalSurfaceResult,
};
pub use crate::terminal::model::block::{
    AgentInteractionMetadata, Block, BlockId, TranscriptScope,
};
pub use crate::terminal::model::blockgrid::BlockGrid;
pub use crate::terminal::model::blocks::{
    BlockHeight, BlockHeightItem, BlockHeightSummary, BlockList, RichContentItem, TotalIndex,
};
pub use crate::terminal::model::escape_sequences::{KeystrokeWithDetails, ToEscapeSequence};
pub use crate::terminal::model::grid::grid_handler::{GridHandler, TermMode};
pub use crate::terminal::model::rich_content::RichContentType;
pub use crate::terminal::model::session::active_session::{ActiveSession, ActiveSessionEvent};
pub use crate::terminal::model::session::{Session, Sessions, SessionsEvent};
pub use crate::terminal::model::terminal_model::BlockIndex;
pub use crate::terminal::model_events::{ModelEvent, ModelEventDispatcher};
pub use crate::terminal::shared_session::IsSharedSessionCreator;
pub use crate::terminal::terminal_manager::BlockSpacing;
pub use crate::terminal::view::ambient_agent::AmbientAgentViewModel;
pub use crate::terminal::view::blocklist_filter::should_show_task_in_blocklist;
pub use crate::terminal::view::{ExecuteCommandEvent, WAKEUP_THROTTLE_PERIOD};
pub use crate::terminal::{
    BlockPadding, PtyIntent, PtyIntentEvent, ShellLaunchData, SizeInfo, SizeUpdate,
    TerminalManager as TerminalManagerTrait, TerminalModel, TerminalSurface,
};
pub use crate::themes::default_themes::{dark_theme, light_theme};
pub use crate::throttle::throttle;
pub use crate::util::repo_detection::{RepoDetectionSessionType, detect_possible_git_repo};
pub use crate::util::time_format::format_elapsed_seconds;

/// Returns whether cloud conversation metadata failed to load.
pub fn agent_conversations_cloud_metadata_load_failed(app: &warpui::AppContext) -> bool {
    crate::ai::agent_conversations_model::AgentConversationsModel::as_ref(app)
        .cloud_conversation_metadata_load_failed()
}

/// Resolves the user-facing name for an MCP server from its installation/template
/// UUID. Returns `None` when the server is unknown (e.g. a legacy/flat MCP call
/// with no server id, or the server is not installed). Used by the TUI to surface
/// tool/server identity in permission cards and transcript labels.
pub fn mcp_server_name_for_id(uuid: &uuid::Uuid, app: &warpui::AppContext) -> Option<String> {
    crate::ai::mcp::TemplatableMCPServerManager::get_mcp_name(uuid, app)
}
