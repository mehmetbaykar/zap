//! Codex CLI harness for local agent runs.
//!
//! Zap trim of upstream `codex.rs`: local-only. The cloud pieces — server
//! transcript upload/resume (`codex_transcript`), external-conversation
//! creation, setup observability, MCP-server and model-config seeding (their
//! inputs aren't in this fork's `ThirdPartyHarness` contract) — are gone.
//! What remains is the config seeding Codex needs to run unattended
//! (auth.json, config.toml trust/update-check/base-url) and a runner that
//! launches the TUI and exits it gracefully, mirroring `gemini.rs`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tempfile::NamedTempFile;
use warp_cli::agent::Harness;
use warp_managed_secrets::ManagedSecretValue;
use warpui::{ModelHandle, ModelSpawner};

use super::super::terminal::{CommandHandle, TerminalDriver};
use super::super::{AgentDriver, AgentDriverError};
use super::json_utils::read_json_file_or_default;
use super::{write_temp_file, HarnessRunner, SavePoint, ThirdPartyHarness};
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent_events::AgentEventStreamClient;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::terminal::model::block::BlockId;
use crate::terminal::CLIAgent;

pub(crate) struct CodexHarness;

/// Slash command Codex's TUI recognises as a graceful shutdown.
const CODEX_EXIT_COMMAND: &str = "/exit";
/// Allow the Warp-installed Codex plugin hooks (see
/// `terminal/cli_agent_sessions/plugin_manager/codex.rs`) to run in driver
/// sessions without an unattended `/hooks` review step.
const CODEX_BYPASS_HOOK_TRUST_FLAG: &str = "--dangerously-bypass-hook-trust";

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl ThirdPartyHarness for CodexHarness {
    fn harness(&self) -> Harness {
        Harness::Codex
    }

    fn cli_agent(&self) -> CLIAgent {
        CLIAgent::Codex
    }

    fn install_docs_url(&self) -> Option<&'static str> {
        Some("https://developers.openai.com/codex/cli")
    }

    fn prepare_environment_config(
        &self,
        working_dir: &Path,
        system_prompt: Option<&str>,
        secrets: &HashMap<String, ManagedSecretValue>,
    ) -> Result<(), AgentDriverError> {
        prepare_codex_environment_config(working_dir, system_prompt, secrets).map_err(|error| {
            AgentDriverError::HarnessConfigSetupFailed {
                harness: self.cli_agent().command_prefix().to_owned(),
                error,
            }
        })
    }

    fn build_runner(
        &self,
        prompt: &str,
        _system_prompt: Option<&str>,
        resumption_prompt: Option<&str>,
        _working_dir: &Path,
        _task_id: Option<AmbientAgentTaskId>,
        _agent_event_stream_client: Arc<dyn AgentEventStreamClient>,
        terminal_driver: ModelHandle<TerminalDriver>,
    ) -> Result<Box<dyn HarnessRunner>, AgentDriverError> {
        // Mirror the Claude harness: prepend the resumption preamble to the
        // user-turn prompt so Codex treats it as immediate intent.
        let owned_prompt = match resumption_prompt {
            Some(preamble) if !preamble.is_empty() => format!("{preamble}\n\n{prompt}"),
            _ => prompt.to_string(),
        };
        Ok(Box::new(CodexHarnessRunner::new(
            self.cli_agent().command_prefix(),
            &owned_prompt,
            terminal_driver,
        )?))
    }
}

/// Build the shell command that launches the Codex TUI.
///
/// `--dangerously-bypass-approvals-and-sandbox` disables both the sandbox and
/// approval prompts so the agent can run autonomously.
/// `--dangerously-bypass-hook-trust` allows the plugin hooks Warp installs to
/// run without a manual hook review in unattended driver sessions.
/// Unlike Claude, Codex does not support assigning a session id to a new
/// conversation, and this fork has no server transcripts to resume from, so
/// every run is a fresh session.
fn codex_command(cli_name: &str, prompt_path: &str) -> String {
    format!(
        "{cli_name} --dangerously-bypass-approvals-and-sandbox {CODEX_BYPASS_HOOK_TRUST_FLAG} \"$(cat '{prompt_path}')\""
    )
}

/// Runtime state of a [`CodexHarnessRunner`].
enum CodexRunnerState {
    /// Runner is built but [`HarnessRunner::start`] has not been called yet.
    Preexec,
    /// The harness command is running (or has finished).
    Running {
        conversation_id: AIConversationId,
        block_id: BlockId,
    },
}

struct CodexHarnessRunner {
    command: String,
    /// Held so the temp file is cleaned up when the runner is dropped.
    _temp_prompt_file: NamedTempFile,
    terminal_driver: ModelHandle<TerminalDriver>,
    state: Mutex<CodexRunnerState>,
}

impl CodexHarnessRunner {
    fn new(
        cli_command: &str,
        prompt: &str,
        terminal_driver: ModelHandle<TerminalDriver>,
    ) -> Result<Self, AgentDriverError> {
        // Write the prompt to a temp file so we can feed it via command
        // substitution, avoiding shell-quoting issues with complex content.
        let temp_file = write_temp_file("oz_prompt_", prompt)?;
        let prompt_path = temp_file.path().display().to_string();

        Ok(Self {
            command: codex_command(cli_command, &prompt_path),
            _temp_prompt_file: temp_file,
            terminal_driver,
            state: Mutex::new(CodexRunnerState::Preexec),
        })
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl HarnessRunner for CodexHarnessRunner {
    async fn start(
        &self,
        foreground: &ModelSpawner<AgentDriver>,
    ) -> Result<CommandHandle, AgentDriverError> {
        let conversation_id = AIConversationId::new();
        log::info!("Created local Codex conversation {conversation_id}");

        let command = self.command.clone();
        let terminal_driver = self.terminal_driver.clone();
        let command_handle = foreground
            .spawn(move |_, ctx| {
                terminal_driver.update(ctx, |driver, ctx| driver.execute_command(&command, ctx))
            })
            .await??
            .await?;

        // Only store conversation info once the CLI command has started.
        *self.state.lock() = CodexRunnerState::Running {
            conversation_id,
            block_id: command_handle.block_id().clone(),
        };

        Ok(command_handle)
    }

    async fn exit(&self, foreground: &ModelSpawner<AgentDriver>) -> Result<()> {
        log::info!("Sending /exit to Codex CLI");
        let terminal_driver = self.terminal_driver.clone();
        foreground
            .spawn(move |_, ctx| {
                terminal_driver.update(ctx, |driver, ctx| {
                    driver.send_text_to_cli(CODEX_EXIT_COMMAND.to_string(), ctx);
                });
            })
            .await
            .map_err(|_| anyhow::anyhow!("Agent driver dropped while sending /exit"))
    }

    async fn save_conversation(
        &self,
        save_point: SavePoint,
        foreground: &ModelSpawner<AgentDriver>,
    ) -> Result<()> {
        if matches!(save_point, SavePoint::Periodic)
            && !super::has_running_cli_agent(&self.terminal_driver, foreground).await
        {
            log::debug!("Will not save conversation, Codex not in progress");
            return Ok(());
        }

        let (conversation_id, block_id) = match &*self.state.lock() {
            CodexRunnerState::Preexec => {
                log::warn!("save_conversation called before start");
                return Ok(());
            }
            CodexRunnerState::Running {
                conversation_id,
                block_id,
            } => (*conversation_id, block_id.clone()),
        };

        let _ = (foreground, conversation_id, block_id);
        log::debug!("Skipping Codex transcript and block snapshot export in Zap");
        Ok(())
    }
}

const CODEX_CONFIG_DIR: &str = ".codex";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const CODEX_AUTH_FILE_NAME: &str = "auth.json";
const CODEX_CONFIG_TOML_FILE_NAME: &str = "config.toml";
const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const CODEX_AUTH_MODE_API_KEY: &str = "apikey";
/// Lowercase string Codex's `TrustLevel` enum serializes to (codex
/// `protocol/src/config_types.rs::TrustLevel`).
const CODEX_TRUST_LEVEL_TRUSTED: &str = "trusted";
/// Top-level config key codex reads to override the built-in `openai` provider's base URL
/// (codex `core/src/config/mod.rs`).
const CODEX_OPENAI_BASE_URL_KEY: &str = "openai_base_url";
const CODEX_CHECK_FOR_UPDATE_ON_STARTUP_KEY: &str = "check_for_update_on_startup";

fn prepare_codex_environment_config(
    working_dir: &Path,
    system_prompt: Option<&str>,
    secrets: &HashMap<String, ManagedSecretValue>,
) -> Result<()> {
    let codex_dir = codex_config_dir()?;

    // Upstream writes the system prompt to `AGENTS.override.md` for cloud runs,
    // where the config dir is disposable. On a user's machine that would clobber
    // their real override file, and the local driver never supplies a system
    // prompt anyway — so it is intentionally not written here.
    if system_prompt.is_some() {
        log::debug!("Ignoring system prompt for local Codex run (no non-clobbering mechanism)");
    }

    match resolve_openai_api_key(secrets) {
        Some(api_key) => prepare_codex_auth(&codex_dir.join(CODEX_AUTH_FILE_NAME), &api_key)?,
        None => log::info!("No OPENAI_API_KEY available; skipping Codex auth.json seed"),
    }

    // Resolve the base URL directly from the typed OpenAI secret. Only applied
    // when that secret is the active API key source (i.e. the process env does
    // not already provide the key).
    let openai_base_url = resolve_openai_base_url_from_secret(secrets);

    prepare_codex_config_toml(
        &codex_dir.join(CODEX_CONFIG_TOML_FILE_NAME),
        working_dir,
        openai_base_url.as_deref(),
    )?;
    Ok(())
}

fn codex_config_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var(CODEX_HOME_ENV) {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    dirs::home_dir()
        .map(|home| home.join(CODEX_CONFIG_DIR))
        .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))
}

/// Mirrors the subset of Codex's `AuthDotJson` (codex `login/src/auth/storage.rs`) that we
/// need to seed. Unknown fields (`tokens`, `last_refresh`, `agent_identity`, ...) are
/// preserved via `extra` so we don't clobber an existing login.
#[derive(Default, Deserialize, Serialize, Debug)]
struct CodexAuthDotJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_mode: Option<String>,
    #[serde(
        rename = "OPENAI_API_KEY",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    openai_api_key: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

fn prepare_codex_auth(auth_path: &Path, api_key: &str) -> Result<()> {
    let mut auth: CodexAuthDotJson = read_json_file_or_default(auth_path)?;
    auth.openai_api_key = Some(api_key.to_owned());
    if auth.auth_mode.is_none() {
        auth.auth_mode = Some(CODEX_AUTH_MODE_API_KEY.to_owned());
    }
    write_codex_auth_json(auth_path, &auth)
}

/// Write Codex's `auth.json` with restrictive (0o600) permissions, mirroring how
/// codex sets up this file itself.
fn write_codex_auth_json(path: &Path, auth: &CodexAuthDotJson) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(auth).context("Failed to serialize Codex auth.json")?;

    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("Failed to open {} for writing", path.display()))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to set permissions on {}", path.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("Failed to write {}", path.display()))?;
    }
    #[cfg(not(unix))]
    fs::write(path, &bytes).with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(())
}

/// Returns the OpenAI API key for Codex auth.
///
/// The process environment wins (a user-exported key is the most explicit
/// intent), then the typed `OpenaiApiKey` secret anywhere in the map, then a
/// `RawValue` stored under the env var name.
fn resolve_openai_api_key(secrets: &HashMap<String, ManagedSecretValue>) -> Option<String> {
    if let Ok(value) = std::env::var(OPENAI_API_KEY_ENV) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    for secret in secrets.values() {
        if let ManagedSecretValue::OpenaiApiKey { api_key, .. } = secret {
            let trimmed = api_key.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    if let Some(ManagedSecretValue::RawValue { value }) = secrets.get(OPENAI_API_KEY_ENV) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    None
}

/// Returns the OpenAI base URL from the typed secret, if applicable.
///
/// Only applied when the typed `OpenaiApiKey` secret is the active source of
/// the API key. If the process env already provides the key, the typed-secret
/// base URL is not applied (whoever set the env controls both key and endpoint).
fn resolve_openai_base_url_from_secret(
    secrets: &HashMap<String, ManagedSecretValue>,
) -> Option<String> {
    if std::env::var(OPENAI_API_KEY_ENV)
        .ok()
        .is_some_and(|v| !v.trim().is_empty())
    {
        return None;
    }

    secrets.values().find_map(|secret| match secret {
        ManagedSecretValue::OpenaiApiKey { base_url, .. } => base_url
            .as_ref()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()),
        _ => None,
    })
}

/// Edit `~/.codex/config.toml` via `toml_edit` to seed the harness defaults
/// while preserving anything that might already exist there. We handle:
/// - project trust: for the working dir and all of its git repo subdirectories,
///   set the projects to `trusted`.
/// - base URL: when `openai_base_url` is provided (from the secret's `base_url`
///   field), write it to config.toml. When absent, skip the key entirely so
///   Codex uses the provider's default global endpoint.
/// - update checks: disable Codex's startup update prompt for unattended runs.
fn prepare_codex_config_toml(
    config_toml_path: &Path,
    working_dir: &Path,
    openai_base_url: Option<&str>,
) -> Result<()> {
    let existing = match fs::read_to_string(config_toml_path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(anyhow::Error::from(e).context(format!(
                "Failed to read Codex config.toml at {}",
                config_toml_path.display()
            )));
        }
    };
    let mut doc: toml_edit::DocumentMut = existing.parse().with_context(|| {
        format!(
            "Failed to parse Codex config.toml at {}",
            config_toml_path.display()
        )
    })?;

    // Only write openai_base_url when the secret specifies one.
    if let Some(url) = openai_base_url {
        set_codex_openai_base_url(&mut doc, url);
    }
    set_codex_check_for_update_on_startup(&mut doc, false);

    let canonical = working_dir.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize Codex working dir at {}",
            working_dir.display()
        )
    })?;
    let project_key = canonical.to_string_lossy().into_owned();
    set_codex_project_trust_level(&mut doc, &project_key, CODEX_TRUST_LEVEL_TRUSTED);

    // Codex's trust check is not recursive (see openai/codex#19426) — trust any
    // direct git-repo children of the working dir too.
    for child_repo in find_child_git_repos(&canonical) {
        let key = child_repo.to_string_lossy().into_owned();
        set_codex_project_trust_level(&mut doc, &key, CODEX_TRUST_LEVEL_TRUSTED);
    }

    if let Some(parent) = config_toml_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create Codex config dir at {}", parent.display())
        })?;
    }
    fs::write(config_toml_path, doc.to_string()).with_context(|| {
        format!(
            "Failed to write Codex config.toml at {}",
            config_toml_path.display()
        )
    })
}

/// Set the top-level `openai_base_url` key, overwriting any existing value.
fn set_codex_openai_base_url(doc: &mut toml_edit::DocumentMut, base_url: &str) {
    doc[CODEX_OPENAI_BASE_URL_KEY] = toml_edit::value(base_url);
}

fn set_codex_check_for_update_on_startup(doc: &mut toml_edit::DocumentMut, enabled: bool) {
    doc[CODEX_CHECK_FOR_UPDATE_ON_STARTUP_KEY] = toml_edit::value(enabled);
}

/// Return immediate subdirectories of `dir` that contain a `.git`.
fn find_child_git_repos(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (path.is_dir() && path.join(".git").exists()).then_some(path)
        })
        .collect()
}

/// Insert/update `[projects."<project_key>"] trust_level = <trust_level>`.
///
/// Codex itself always writes `projects` as an explicit table, so we don't
/// handle the inline-table form here.
fn set_codex_project_trust_level(
    doc: &mut toml_edit::DocumentMut,
    project_key: &str,
    trust_level: &str,
) {
    if !doc.contains_table("projects") {
        let mut projects_tbl = toml_edit::Table::new();
        projects_tbl.set_implicit(true);
        doc.insert("projects", toml_edit::Item::Table(projects_tbl));
    }
    let proj_tbl = doc["projects"]
        .as_table_mut()
        .expect("projects table inserted above")
        .entry(project_key)
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .expect("project entry is a table");
    proj_tbl.set_implicit(false);
    proj_tbl["trust_level"] = toml_edit::value(trust_level);
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
