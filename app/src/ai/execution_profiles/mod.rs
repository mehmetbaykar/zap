use std::path::PathBuf;

use markdown_parser::{FormattedTextFragment, FormattedTextInline};
use serde::{Deserialize, Serialize};
use warp_core::channel::ChannelState;
use warp_core::features::FeatureFlag;
use warpui::{AppContext, SingletonEntity};

use super::llms::{LLMContextWindow, LLMId, LLMInfo, LLMPreferences, LLMProvider};
use crate::cloud_object::model::generic_string_model::{
    GenericStringModel, GenericStringObjectId, StringModel,
};
use crate::cloud_object::model::json_model::{JsonModel, JsonSerializer};
use crate::cloud_object::{
    GenericStoredObject, GenericStringObjectFormat, GenericStringObjectUniqueKey, JsonObjectType,
    UniquePer,
};
use crate::settings::{
    AISettings, AgentModeCommandExecutionPredicate, DEFAULT_COMMAND_EXECUTION_ALLOWLIST,
    DEFAULT_COMMAND_EXECUTION_DENYLIST,
};

pub const PROFILE_NAME_MAX_LENGTH: usize = 50;
/// This threshold currently only applies to GPT 5.4 and GPT 5.5 models
pub const LONG_CONTEXT_WARNING_THRESHOLD: u32 = 272_000;
pub(crate) const LONG_CONTEXT_PRICING_WARNING_URL: &str =
    "https://developers.openai.com/api/docs/pricing";
pub(crate) fn long_context_pricing_warning_title() -> FormattedTextInline {
    vec![
        FormattedTextFragment::plain_text(
            "OpenAI automatically applies long-context pricing when context exceeds 272,000 tokens. ",
        ),
        FormattedTextFragment::hyperlink("Learn more", LONG_CONTEXT_PRICING_WARNING_URL),
    ]
}

mod config;
pub mod editor;
pub mod model_menu_items;
pub mod profiles;
pub use config::{ExecutionProfileId, ExecutionProfilesConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionPermission {
    AgentDecides,
    AlwaysAllow,
    AlwaysAsk,

    // This is intended to catch deserialization errors whenever we add new variants to this enum. Say we
    // want to add a "Never" variant. Without this catch-all, old clients wouldn't be able to deserialize
    // a "Never" into one of the existing options.
    #[serde(other)]
    Unknown,
}
fn effective_base_model<'a>(profile: &AIExecutionProfile, app: &'a AppContext) -> &'a LLMInfo {
    let prefs = LLMPreferences::as_ref(app);
    profile
        .base_model
        .as_ref()
        .and_then(|id| prefs.get_llm_info(id))
        .unwrap_or_else(|| prefs.get_default_base_model())
}

impl ActionPermission {
    pub fn description(&self) -> &'static str {
        match self {
            ActionPermission::AgentDecides | ActionPermission::Unknown => {
                "The Agent chooses the safest path: acting on its own when confident, and asking for approval when uncertain."
            }
            ActionPermission::AlwaysAllow => {
                "Give the Agent full autonomy  — no manual approval ever required."
            }
            ActionPermission::AlwaysAsk => {
                "Require explicit approval before the Agent takes any action."
            }
        }
    }

    pub fn is_always_ask(&self) -> bool {
        matches!(self, Self::AlwaysAsk)
    }

    pub fn is_always_allow(&self) -> bool {
        matches!(self, Self::AlwaysAllow)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteToPtyPermission {
    // This is for backwards compatibility with the old "Never" value.
    #[serde(alias = "Never")]
    AlwaysAllow,
    #[default]
    AlwaysAsk,
    AskOnFirstWrite,

    // This is intended to catch deserialization errors whenever we add new variants to this enum.
    #[serde(other)]
    Unknown,
}

impl WriteToPtyPermission {
    pub fn description(&self) -> &'static str {
        match self {
            WriteToPtyPermission::AlwaysAllow => ActionPermission::AlwaysAllow.description(),
            WriteToPtyPermission::AskOnFirstWrite => {
                "The agent will ask for permission the first time it needs to interact with a running command. After that, it will continue automatically for the rest of that command."
            }
            WriteToPtyPermission::AlwaysAsk => {
                "The agent will always ask for permission to interact with a running command."
            }
            WriteToPtyPermission::Unknown => ActionPermission::Unknown.description(),
        }
    }

    pub fn is_always_allow(&self) -> bool {
        matches!(self, Self::AlwaysAllow)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComputerUsePermission {
    #[default]
    Never,
    AlwaysAsk,
    AlwaysAllow,

    // This is intended to catch deserialization errors whenever we add new variants to this enum.
    #[serde(other)]
    Unknown,
}

impl ComputerUsePermission {
    pub fn description(&self) -> &'static str {
        match self {
            ComputerUsePermission::Never => {
                "Computer use tools are disabled and will not be available to the Agent."
            }
            ComputerUsePermission::AlwaysAsk => {
                "Require explicit approval before the Agent uses computer use tools."
            }
            ComputerUsePermission::AlwaysAllow => {
                "Give the Agent full autonomy to use computer use tools without approval."
            }
            ComputerUsePermission::Unknown => "Unknown setting.",
        }
    }

    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Never | Self::Unknown)
    }

    pub fn is_always_allow(&self) -> bool {
        matches!(self, Self::AlwaysAllow)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AskUserQuestionPermission {
    /// Never pause; skip questions and continue with best judgment.
    Never,
    /// In openWarp this is equivalent to `AlwaysAsk`: auto-approve mode no longer silently skips user questions,
    /// and only auto-passes execution-class tools like shell/edit. The variant name is kept for compatibility with serialized profiles.
    #[default]
    AskExceptInAutoApprove,
    /// Always pause and wait for the user to answer before continuing, even in auto-approve mode.
    AlwaysAsk,

    // This is intended to catch deserialization errors whenever we add new variants to this enum.
    #[serde(other)]
    Unknown,
}

impl AskUserQuestionPermission {
    pub fn label(&self) -> &'static str {
        match self {
            AskUserQuestionPermission::Never => "Never ask",
            AskUserQuestionPermission::AskExceptInAutoApprove => "Ask unless auto-approve",
            AskUserQuestionPermission::AlwaysAsk | AskUserQuestionPermission::Unknown => {
                "Always ask"
            }
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            AskUserQuestionPermission::AskExceptInAutoApprove
            | AskUserQuestionPermission::Unknown => {
                "The Agent may ask a question and will pause for your response, even when auto-approve is on (auto-approve only applies to shell/edit tools)."
            }
            AskUserQuestionPermission::Never => {
                "The Agent will not ask questions and will continue with its best judgment."
            }
            AskUserQuestionPermission::AlwaysAsk => {
                "The Agent may ask a question and will pause for your response even when auto-approve is on."
            }
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunAgentsPermission {
    NeverAllow,
    AlwaysAllow,
    #[default]
    AlwaysAsk,

    // This is intended to catch deserialization errors whenever we add new variants to this enum.
    #[serde(other)]
    Unknown,
}

impl RunAgentsPermission {
    pub fn description(&self) -> &'static str {
        match self {
            RunAgentsPermission::NeverAllow => {
                "The Agent cannot run child agents and the run_agents tool will not be available."
            }
            RunAgentsPermission::AlwaysAllow => {
                "Give the Agent full autonomy to run child agents without approval."
            }
            RunAgentsPermission::AlwaysAsk => {
                "Require explicit approval before the Agent runs child agents."
            }
            RunAgentsPermission::Unknown => "Unknown setting.",
        }
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::AlwaysAllow | Self::AlwaysAsk)
    }

    pub fn is_always_allow(&self) -> bool {
        matches!(self, Self::AlwaysAllow)
    }

    pub fn is_never_allow(&self) -> bool {
        matches!(self, Self::NeverAllow | Self::Unknown)
    }
}

/// Core data structure representing an AI execution profile, which includes model configuration,
/// behavior settings, and permissions.
///
/// NOTE: `planning_model` was removed after planning via subagent was deprecated; serialized legacy
/// profiles may include a `planning_model` field and this field name should remain reserved
/// indefinitely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AIExecutionProfile {
    pub name: String,
    pub is_default_profile: bool,
    pub apply_code_diffs: ActionPermission,
    pub read_files: ActionPermission,

    pub execute_commands: ActionPermission,
    pub write_to_pty: WriteToPtyPermission,
    pub mcp_permissions: ActionPermission,
    pub ask_user_question: AskUserQuestionPermission,
    pub run_agents: RunAgentsPermission,

    /// Always ask for permission for these commands
    pub command_denylist: Vec<AgentModeCommandExecutionPredicate>,

    /// When the execute_commands is set to AlwaysAsk, autoexecute these commands
    pub command_allowlist: Vec<AgentModeCommandExecutionPredicate>,

    /// When the read_files is set to AlwaysAsk, autoread from these directories
    pub directory_allowlist: Vec<PathBuf>,

    pub mcp_allowlist: Vec<uuid::Uuid>,
    pub mcp_denylist: Vec<uuid::Uuid>,

    pub computer_use: ComputerUsePermission,

    pub base_model: Option<LLMId>,
    pub coding_model: Option<LLMId>,
    pub cli_agent_model: Option<LLMId>,
    pub computer_use_model: Option<LLMId>,
    /// The model used to generate conversation titles. Falls back to `base_model` when `None`.
    pub title_model: Option<LLMId>,
    /// The model used for proactive AI (prompt suggestions / NLD / relevant files).
    /// Falls back to `base_model` when `None`. A small/fast/cheap BYOP model is recommended.
    pub active_ai_model: Option<LLMId>,
    /// The model used for Next Command (gray completion / zero-state suggestions).
    /// Falls back to `base_model` when `None`. Latency-sensitive, so the cheapest/fastest BYOP model is recommended.
    pub next_command_model: Option<LLMId>,

    pub context_window_limit: Option<u32>,

    /// Whether the agent may use web search when helpful for completing tasks
    pub web_search_enabled: bool,
}

impl Default for AIExecutionProfile {
    fn default() -> Self {
        Self {
            name: Default::default(),
            is_default_profile: false,
            apply_code_diffs: ActionPermission::AgentDecides,
            read_files: ActionPermission::AgentDecides,
            execute_commands: ActionPermission::AlwaysAsk,
            write_to_pty: WriteToPtyPermission::AlwaysAsk,
            mcp_permissions: ActionPermission::AgentDecides,
            ask_user_question: AskUserQuestionPermission::AlwaysAsk,
            run_agents: RunAgentsPermission::AlwaysAsk,
            command_denylist: DEFAULT_COMMAND_EXECUTION_DENYLIST.clone(),
            command_allowlist: Vec::new(),
            directory_allowlist: Vec::new(),
            mcp_allowlist: Vec::new(),
            mcp_denylist: Vec::new(),
            computer_use: ComputerUsePermission::Never,
            base_model: None,
            coding_model: None,
            cli_agent_model: None,
            computer_use_model: None,
            title_model: None,
            active_ai_model: None,
            next_command_model: None,
            context_window_limit: None,
            web_search_enabled: true,
        }
    }
}

impl AIExecutionProfile {
    pub fn create_default_from_legacy_settings(app: &AppContext) -> Self {
        // Note that the legacy "Autonomy" and "Code Access" settings are not imported here.
        // The "Code Access" setting defaulted to "Always Ask", which is the most restrictive, so
        // it's impossible for us to infer some hesitancy about autonomy from the setting and we should
        // ignore it. The same applies to "Autonomy".
        let ai_settings = AISettings::as_ref(app);
        Self {
            name: "Default".to_string(),
            is_default_profile: true,
            command_denylist: ai_settings.agent_mode_command_execution_denylist.clone(),
            // We initialize the command allowlist to be anything the user added, excluding all
            // the pre-populated defaults.
            command_allowlist: ai_settings
                .agent_mode_command_execution_allowlist
                .iter()
                .filter(|cmd| !DEFAULT_COMMAND_EXECUTION_ALLOWLIST.contains(cmd))
                .cloned()
                .collect(),
            directory_allowlist: ai_settings.agent_mode_coding_file_read_allowlist.clone(),
            ..Default::default()
        }
    }

    #[cfg(feature = "agent_mode_evals")]
    pub fn create_agent_mode_eval_profile() -> Self {
        Self {
            name: "Agent Mode Eval".to_string(),
            is_default_profile: false,
            apply_code_diffs: ActionPermission::AlwaysAllow,
            read_files: ActionPermission::AlwaysAllow,
            execute_commands: ActionPermission::AlwaysAllow,
            write_to_pty: WriteToPtyPermission::AlwaysAllow,
            mcp_permissions: ActionPermission::AlwaysAllow,
            ask_user_question: AskUserQuestionPermission::Never,
            run_agents: RunAgentsPermission::AlwaysAllow,
            command_denylist: Vec::new(),
            command_allowlist: Vec::new(),
            directory_allowlist: Vec::new(),
            mcp_allowlist: Vec::new(),
            mcp_denylist: Vec::new(),
            computer_use: ComputerUsePermission::Never,
            base_model: None,
            coding_model: None,
            cli_agent_model: None,
            computer_use_model: None,
            title_model: None,
            active_ai_model: None,
            next_command_model: None,
            context_window_limit: None,
            web_search_enabled: true,
        }
    }

    /// This creates a CLI-specific profile that will never ask the user for permission,
    /// since we cannot do so in a non-interactive setting.
    pub fn create_default_cli_profile(
        is_sandboxed: bool,
        computer_use_override: Option<bool>,
    ) -> Self {
        let command_denylist = if is_sandboxed {
            Vec::new()
        } else {
            DEFAULT_COMMAND_EXECUTION_DENYLIST.to_vec()
        };

        let computer_use_permission = match computer_use_override {
            Some(true) => {
                if is_sandboxed || FeatureFlag::LocalComputerUse.is_enabled() {
                    ComputerUsePermission::AlwaysAllow
                } else {
                    ComputerUsePermission::Never
                }
            }
            Some(false) => ComputerUsePermission::Never,
            None => {
                if is_sandboxed && ChannelState::channel().is_dogfood() {
                    ComputerUsePermission::AlwaysAllow
                } else {
                    ComputerUsePermission::Never
                }
            }
        };

        Self {
            name: "Default (CLI)".to_owned(),
            is_default_profile: true,
            apply_code_diffs: ActionPermission::AlwaysAllow,
            read_files: ActionPermission::AlwaysAllow,
            execute_commands: ActionPermission::AlwaysAllow,
            mcp_permissions: ActionPermission::AlwaysAllow,
            write_to_pty: WriteToPtyPermission::AlwaysAllow,
            ask_user_question: AskUserQuestionPermission::Never,
            run_agents: RunAgentsPermission::AlwaysAllow,
            command_denylist,
            command_allowlist: DEFAULT_COMMAND_EXECUTION_ALLOWLIST.to_vec(),
            directory_allowlist: Vec::new(),
            mcp_allowlist: Vec::new(),
            mcp_denylist: Vec::new(),
            computer_use: computer_use_permission,
            base_model: None,
            coding_model: None,
            cli_agent_model: None,
            computer_use_model: None,
            title_model: None,
            active_ai_model: None,
            next_command_model: None,
            context_window_limit: None,
            web_search_enabled: true,
        }
    }
}

pub trait AIExecutionProfileAppExt {
    fn configurable_context_window(&self, app: &AppContext) -> Option<LLMContextWindow>;
    fn context_window_display_value(&self, app: &AppContext) -> Option<u32>;
    fn context_window_limit_for_request(&self, app: &AppContext) -> Option<u32>;
    fn should_show_long_context_pricing_warning(
        &self,
        context_window_limit: Option<u32>,
        app: &AppContext,
    ) -> bool;
}

impl AIExecutionProfileAppExt for AIExecutionProfile {
    fn configurable_context_window(&self, app: &AppContext) -> Option<LLMContextWindow> {
        let llm = effective_base_model(self, app);
        if has_configurable_context_window(
            llm,
            FeatureFlag::GPTConfigurableContextWindow.is_enabled(),
        ) {
            Some(llm.context_window.clone())
        } else {
            None
        }
    }

    fn context_window_display_value(&self, app: &AppContext) -> Option<u32> {
        let cw = self.configurable_context_window(app)?;
        Some(self.context_window_limit.unwrap_or(cw.default_max))
    }
    fn context_window_limit_for_request(&self, app: &AppContext) -> Option<u32> {
        let llm = effective_base_model(self, app);
        if !has_configurable_context_window(
            llm,
            FeatureFlag::GPTConfigurableContextWindow.is_enabled(),
        ) {
            return None;
        }

        self.context_window_limit
            .map(|limit| limit.clamp(llm.context_window.min, llm.context_window.max))
    }

    fn should_show_long_context_pricing_warning(
        &self,
        context_window_limit: Option<u32>,
        app: &AppContext,
    ) -> bool {
        let llm = effective_base_model(self, app);
        should_show_long_context_pricing_warning(
            llm,
            Some(
                context_window_limit
                    .or(self.context_window_limit)
                    .unwrap_or(llm.context_window.default_max),
            ),
            FeatureFlag::GPTConfigurableContextWindow.is_enabled(),
        )
    }
}

pub(crate) fn has_configurable_context_window(
    llm: &LLMInfo,
    gpt_configurable_context_window_enabled: bool,
) -> bool {
    llm.context_window.is_configurable
        && llm.context_window.max > 0
        && (llm.provider != LLMProvider::OpenAI || gpt_configurable_context_window_enabled)
}

pub(crate) fn should_show_long_context_pricing_warning(
    llm: &LLMInfo,
    selected_limit: Option<u32>,
    gpt_configurable_context_window_enabled: bool,
) -> bool {
    llm.provider == LLMProvider::OpenAI
        && has_configurable_context_window(llm, gpt_configurable_context_window_enabled)
        && selected_limit
            .map(|limit| limit.clamp(llm.context_window.min, llm.context_window.max))
            .is_some_and(|limit| limit > LONG_CONTEXT_WARNING_THRESHOLD)
}

pub type AIExecutionProfileObject =
    GenericStoredObject<GenericStringObjectId, AIExecutionProfileObjectModel>;
pub type AIExecutionProfileObjectModel = GenericStringModel<AIExecutionProfile, JsonSerializer>;

impl StringModel for AIExecutionProfile {
    type StoredObjectType = AIExecutionProfileObject;

    fn model_type_name(&self) -> &'static str {
        "AIExecutionProfile"
    }

    fn should_enforce_revisions() -> bool {
        true
    }

    fn model_format() -> GenericStringObjectFormat {
        GenericStringObjectFormat::Json(JsonObjectType::AIExecutionProfile)
    }

    fn should_show_activity_toasts() -> bool {
        false
    }

    fn warn_if_unsaved_at_quit() -> bool {
        true
    }

    fn display_name(&self) -> String {
        // Handles case where default profile was previously created and named "Untitled"
        if self.is_default_profile {
            "Default".to_string()
        } else if self.name.trim().is_empty() {
            "Untitled".to_string()
        } else {
            self.name.clone()
        }
    }

    fn should_clear_on_unique_key_conflict(&self) -> bool {
        true
    }

    fn uniqueness_key(&self) -> Option<GenericStringObjectUniqueKey> {
        // We want to prevent the creation of several default profiles per user. If it's not the default
        // profile, then there can be many.
        self.is_default_profile
            .then_some(GenericStringObjectUniqueKey {
                key: "default".to_string(),
                unique_per: UniquePer::User,
            })
    }

    fn renders_in_warp_drive(&self) -> bool {
        false
    }
}

impl JsonModel for AIExecutionProfile {
    fn json_object_type() -> JsonObjectType {
        JsonObjectType::AIExecutionProfile
    }
}
