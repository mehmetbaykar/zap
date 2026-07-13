use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(feature = "local_fs")]
use std::sync::Arc;

use chrono::{DateTime, Utc};
use itertools::Itertools;
use lsp::supported_servers::LSPServerType;
use lsp::LanguageId;
#[cfg(feature = "local_fs")]
use lsp::{LspEvent, LspManagerModel, LspServerConfig};
#[cfg(feature = "local_fs")]
use repo_metadata::RepoMetadataModel;
use serde::{Deserialize, Serialize};
#[cfg(feature = "local_fs")]
use warp_core::channel::ChannelState;
#[cfg(feature = "local_fs")]
use warp_util::standardized_path::StandardizedPath;
#[cfg(feature = "local_fs")]
use warpui::windowing::WindowManager;
use warpui::{Entity, ModelContext, SingletonEntity};

#[cfg(feature = "local_fs")]
use crate::code::language_server_shutdown_manager::LanguageServerShutdownManager;
#[cfg(feature = "local_fs")]
use crate::terminal::local_shell::LocalShellState;
#[cfg(feature = "local_fs")]
use crate::{view_components::DismissibleToast, workspace::ToastStack};

#[derive(Debug, Default, Clone)]
pub struct WorkspaceMetadata {
    pub path: PathBuf,
    pub navigated_ts: Option<DateTime<Utc>>,
    pub modified_ts: Option<DateTime<Utc>>,
    pub queried_ts: Option<DateTime<Utc>>,
}

impl WorkspaceMetadata {
    pub fn most_recently_touched(a: &Self, b: &Self) -> std::cmp::Ordering {
        match (a.last_touched(), b.last_touched()) {
            (Some(a_ts), Some(b_ts)) => b_ts.cmp(&a_ts),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.path.cmp(&b.path),
        }
    }

    pub fn last_touched(&self) -> Option<DateTime<Utc>> {
        [self.navigated_ts, self.modified_ts, self.queried_ts]
            .into_iter()
            .flatten()
            .max()
    }
}

/// Whether a language server is enabled for a local workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnablementState {
    Yes,
    No,
    /// The server is relevant to the workspace, but the user has not enabled it.
    Suggested,
}

/// An LSP operation that needs the user's interactive shell PATH.
#[cfg(feature = "local_fs")]
pub enum LspTask {
    Install {
        file_path: PathBuf,
        repo_root: PathBuf,
        server_type: LSPServerType,
    },
    Spawn {
        file_path: PathBuf,
    },
}

pub enum LSPEnablementResultForFile {
    Enabled,
    UnsupportedLanguage,
    LSPNotEnabled { root_name: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LspRepoStatus {
    Ready,
    Enabled,
    CheckingForInstallation,
    DisabledAndInstalled { server_type: LSPServerType },
    DisabledAndNotInstalled { server_type: LSPServerType },
    Installing { server_type: LSPServerType },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LSPInstallationStatus {
    Installed,
    NotInstalled,
    Checking,
    Installing,
}

impl LspRepoStatus {
    pub fn from_installation_status(
        status: &LSPInstallationStatus,
        server_type: LSPServerType,
    ) -> Self {
        match status {
            LSPInstallationStatus::Installed => Self::DisabledAndInstalled { server_type },
            LSPInstallationStatus::NotInstalled => Self::DisabledAndNotInstalled { server_type },
            LSPInstallationStatus::Checking => Self::CheckingForInstallation,
            LSPInstallationStatus::Installing => Self::Installing { server_type },
        }
    }
}

struct Workspace {
    metadata: WorkspaceMetadata,
    language_servers: HashMap<LSPServerType, EnablementState>,
}

impl Workspace {
    fn is_user_visible(&self) -> bool {
        self.metadata.navigated_ts.is_some()
            || self.metadata.modified_ts.is_some()
            || self.metadata.queried_ts.is_some()
    }
}

/// Local-only workspace and language-server lifecycle state.
///
/// Upstream couples this model to Warp's codebase-index service and SQLite
/// metadata. Zap deliberately keeps only the local LSP behavior here. The
/// state is rebuilt from local workspace discovery during each app session.
pub struct PersistedWorkspace {
    workspaces: HashMap<PathBuf, Workspace>,
    #[cfg(feature = "local_fs")]
    lsp_installation_status: HashMap<LSPServerType, LSPInstallationStatus>,
}

#[derive(Debug, Clone)]
pub enum PersistedWorkspaceEvent {
    InstallStatusUpdate {
        server_type: LSPServerType,
        status: LSPInstallationStatus,
    },
    InstallationSucceeded,
    InstallationFailed,
    AvailableServersDetected {
        workspace_path: PathBuf,
        servers: Vec<LSPServerType>,
    },
    WorkspaceAdded {
        path: PathBuf,
    },
}

impl Entity for PersistedWorkspace {
    type Event = PersistedWorkspaceEvent;
}

impl SingletonEntity for PersistedWorkspace {}

impl PersistedWorkspace {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            workspaces: HashMap::new(),
            #[cfg(feature = "local_fs")]
            lsp_installation_status: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub fn new_for_test(ctx: &mut ModelContext<Self>) -> Self {
        Self::new(ctx)
    }

    fn ensure_workspace(&mut self, path: &Path, user_visible: bool) {
        let workspace = self
            .workspaces
            .entry(path.to_path_buf())
            .or_insert_with(|| Workspace {
                metadata: WorkspaceMetadata {
                    path: path.to_path_buf(),
                    navigated_ts: None,
                    modified_ts: None,
                    queried_ts: None,
                },
                language_servers: HashMap::new(),
            });
        if user_visible && !workspace.is_user_visible() {
            workspace.metadata.navigated_ts = Some(Utc::now());
        }
    }

    pub fn enable_lsp_server_for_path(&mut self, path: &Path, server_type: LSPServerType) {
        self.set_lsp_server_for_path(path, server_type, EnablementState::Yes);
    }

    pub fn disable_lsp_server_for_path(&mut self, path: &Path, server_type: LSPServerType) {
        self.set_lsp_server_for_path(path, server_type, EnablementState::No);
    }

    fn set_lsp_server_for_path(
        &mut self,
        path: &Path,
        server_type: LSPServerType,
        state: EnablementState,
    ) {
        self.ensure_workspace(path, true);
        let workspace = self
            .workspaces
            .get_mut(path)
            .expect("workspace was inserted above");
        workspace.metadata.modified_ts = Some(Utc::now());
        workspace.language_servers.insert(server_type, state);
    }

    pub fn has_enabled_lsp_server_for_file_path(&self, path: &Path) -> LSPEnablementResultForFile {
        let Some(language_id) = LanguageId::from_path(path) else {
            return LSPEnablementResultForFile::UnsupportedLanguage;
        };
        let Some(root) = self.root_for_workspace(path) else {
            return LSPEnablementResultForFile::LSPNotEnabled { root_name: None };
        };
        let Some(workspace) = self.workspaces.get(root) else {
            return LSPEnablementResultForFile::LSPNotEnabled { root_name: None };
        };

        if workspace.language_servers.iter().any(|(server, state)| {
            *state == EnablementState::Yes && server.languages().contains(&language_id)
        }) {
            LSPEnablementResultForFile::Enabled
        } else {
            LSPEnablementResultForFile::LSPNotEnabled {
                root_name: root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned),
            }
        }
    }

    pub fn root_for_workspace<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.ancestors()
            .find(|ancestor| self.workspaces.contains_key(*ancestor))
    }

    pub fn enabled_lsp_servers(
        &self,
        path: &Path,
    ) -> Option<impl Iterator<Item = LSPServerType> + use<'_>> {
        let root = self.root_for_workspace(path)?;
        self.workspaces.get(root).map(|workspace| {
            workspace
                .language_servers
                .iter()
                .filter_map(|(server, state)| (*state == EnablementState::Yes).then_some(*server))
        })
    }

    pub fn all_lsp_servers(
        &self,
        path: &Path,
        include_suggested: bool,
    ) -> Option<impl Iterator<Item = (LSPServerType, EnablementState)> + use<'_>> {
        let root = self.root_for_workspace(path)?;
        self.workspaces.get(root).map(move |workspace| {
            workspace
                .language_servers
                .iter()
                .filter(move |(_, state)| {
                    include_suggested || **state != EnablementState::Suggested
                })
                .map(|(server, state)| (*server, *state))
        })
    }

    pub fn total_lsp_server_count(&self, include_suggested: bool) -> usize {
        self.workspaces
            .values()
            .map(|workspace| {
                workspace
                    .language_servers
                    .values()
                    .filter(|state| include_suggested || **state != EnablementState::Suggested)
                    .count()
            })
            .sum()
    }

    pub fn user_added_workspace(&mut self, path: PathBuf, ctx: &mut ModelContext<Self>) {
        self.ensure_workspace(&path, true);
        #[cfg(feature = "local_fs")]
        match StandardizedPath::from_local_canonicalized(&path) {
            Ok(standardized_path) => {
                if let Err(error) = RepoMetadataModel::handle(ctx).update(ctx, |model, ctx| {
                    model.index_local_directory_path(&standardized_path, ctx)
                }) {
                    log::warn!("Failed to index user-added workspace {standardized_path}: {error}");
                }
            }
            Err(error) => {
                log::warn!(
                    "Failed to canonicalize user-added workspace {}: {error}",
                    path.display()
                );
            }
        }
        ctx.emit(PersistedWorkspaceEvent::WorkspaceAdded { path });
    }

    pub fn workspaces(&self) -> impl Iterator<Item = WorkspaceMetadata> + use<'_> {
        self.workspaces
            .values()
            .filter(|workspace| workspace.is_user_visible())
            .map(|workspace| workspace.metadata.clone())
            .sorted_by(WorkspaceMetadata::most_recently_touched)
            .dedup_by(|left, right| left.path == right.path)
    }

    pub fn navigated_to_path(&mut self, directory: &PathBuf) {
        self.ensure_workspace(directory, true);
    }

    pub fn workspace_for_path(&self, root_path: &Path) -> Option<WorkspaceMetadata> {
        self.workspaces
            .get(root_path)
            .map(|workspace| workspace.metadata.clone())
    }

    #[cfg(feature = "local_fs")]
    pub fn detect_available_servers_for_workspaces(
        &mut self,
        workspace_paths: Vec<PathBuf>,
        skip_cached: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let mut paths_to_scan = Vec::new();
        for workspace_path in workspace_paths {
            self.ensure_workspace(&workspace_path, false);
            let cached = self
                .workspaces
                .get(&workspace_path)
                .map(|workspace| {
                    workspace
                        .language_servers
                        .keys()
                        .copied()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if !skip_cached && !cached.is_empty() {
                ctx.emit(PersistedWorkspaceEvent::AvailableServersDetected {
                    workspace_path,
                    servers: cached,
                });
            } else {
                paths_to_scan.push(workspace_path);
            }
        }
        if paths_to_scan.is_empty() {
            return;
        }

        let path_future = LocalShellState::handle(ctx).update(ctx, |shell_state, ctx| {
            shell_state.get_interactive_path_env_var(ctx)
        });
        let http_client = Arc::new(http_client::Client::new());
        ctx.spawn(
            async move {
                let executor = lsp::CommandBuilder::new(path_future.await);
                let mut results = Vec::new();
                for workspace_path in paths_to_scan {
                    let mut servers = Vec::new();
                    for server_type in LSPServerType::all() {
                        if server_type
                            .candidate(http_client.clone())
                            .should_suggest_for_repo(&workspace_path, &executor)
                            .await
                        {
                            servers.push(server_type);
                        }
                    }
                    results.push((workspace_path, servers));
                }
                results
            },
            |me, results, ctx| {
                for (workspace_path, servers) in results {
                    let workspace = me
                        .workspaces
                        .get_mut(&workspace_path)
                        .expect("workspace was inserted before detection");
                    for server_type in &servers {
                        workspace
                            .language_servers
                            .entry(*server_type)
                            .or_insert(EnablementState::Suggested);
                    }
                    ctx.emit(PersistedWorkspaceEvent::AvailableServersDetected {
                        workspace_path,
                        servers,
                    });
                }
            },
        );
    }

    #[cfg(feature = "local_fs")]
    fn handle_install_lsp(
        &mut self,
        file_path: PathBuf,
        repo_root: PathBuf,
        server_type: LSPServerType,
        path_env_var: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.lsp_installation_status.get(&server_type)
            == Some(&LSPInstallationStatus::Installing)
        {
            return;
        }
        self.lsp_installation_status
            .insert(server_type, LSPInstallationStatus::Installing);
        ctx.emit(PersistedWorkspaceEvent::InstallStatusUpdate {
            server_type,
            status: LSPInstallationStatus::Installing,
        });

        let executor = lsp::CommandBuilder::new(path_env_var);
        let http_client = Arc::new(http_client::Client::new());
        ctx.spawn(
            async move {
                let candidate = server_type.candidate(http_client);
                let metadata = candidate.fetch_latest_server_metadata().await?;
                candidate.install(metadata, &executor).await
            },
            move |me, result, ctx| match result {
                Ok(()) => {
                    me.enable_lsp_server_for_path(&repo_root, server_type);
                    me.lsp_installation_status
                        .insert(server_type, LSPInstallationStatus::Installed);
                    ctx.emit(PersistedWorkspaceEvent::InstallationSucceeded);
                    ctx.emit(PersistedWorkspaceEvent::InstallStatusUpdate {
                        server_type,
                        status: LSPInstallationStatus::Installed,
                    });
                    me.execute_lsp_task(LspTask::Spawn { file_path }, ctx);
                }
                Err(error) => {
                    log::info!("Failed to install LSP server: {error}");
                    me.lsp_installation_status
                        .insert(server_type, LSPInstallationStatus::NotInstalled);
                    if let Some(window_id) = WindowManager::as_ref(ctx).active_window() {
                        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                            toast_stack.add_ephemeral_toast(
                                DismissibleToast::error(format!(
                                    "Failed to install {}: {error}",
                                    server_type.binary_name()
                                )),
                                window_id,
                                ctx,
                            );
                        });
                    }
                    ctx.emit(PersistedWorkspaceEvent::InstallationFailed);
                    ctx.emit(PersistedWorkspaceEvent::InstallStatusUpdate {
                        server_type,
                        status: LSPInstallationStatus::NotInstalled,
                    });
                }
            },
        );
    }

    #[cfg(feature = "local_fs")]
    fn handle_spawn_lsp(
        &self,
        file_path: &Path,
        path_env_var: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(workspace_root) = self.root_for_workspace(file_path).map(Path::to_path_buf) else {
            return;
        };
        let Some(servers) = self.enabled_lsp_servers(&workspace_root) else {
            return;
        };
        let servers = servers.collect::<Vec<_>>();
        if servers.is_empty() {
            return;
        }

        let mut should_start = false;
        for server_type in servers {
            if LspManagerModel::as_ref(ctx).server_registered_and_started(
                &workspace_root,
                server_type,
                ctx,
            ) {
                continue;
            }
            let config = LspServerConfig::new(
                server_type,
                workspace_root.clone(),
                path_env_var.clone(),
                ChannelState::app_id().application_name().to_string(),
                Arc::new(http_client::Client::new()),
            )
            .with_log_relative_path(crate::code::lsp_logs::relative_log_path(
                server_type,
                &workspace_root,
            ));
            LspManagerModel::handle(ctx).update(ctx, |manager, ctx| {
                manager.register(workspace_root.clone(), config, ctx);
            });
            should_start = true;
        }
        if !should_start {
            return;
        }

        let lsp_manager = LspManagerModel::handle(ctx);
        lsp_manager.update(ctx, |manager, ctx| {
            manager.start_all(workspace_root.clone(), ctx);
        });
        let workspace_display = workspace_root.display().to_string();
        let server_handles = lsp_manager
            .as_ref(ctx)
            .servers_for_workspace(&workspace_root)
            .cloned()
            .unwrap_or_default();
        for server in server_handles {
            let workspace_display = workspace_display.clone();
            ctx.subscribe_to_model(&server, move |_, event, ctx| {
                if let LspEvent::Failed(error) = event {
                    log::warn!(
                        "Failed to start LSP server for {workspace_display}: {error}"
                    );
                    if let Some(window_id) = WindowManager::as_ref(ctx).active_window() {
                        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                            toast_stack.add_ephemeral_toast(
                                DismissibleToast::error(format!(
                                    "Failed to start language server for {workspace_display}: {error}"
                                )),
                                window_id,
                                ctx,
                            );
                        });
                    }
                }
            });
        }
        LanguageServerShutdownManager::handle(ctx).update(ctx, |manager, ctx| {
            if !manager.has_in_progress_scan() {
                manager.schedule_next_scan(ctx);
            }
        });
    }

    #[cfg(feature = "local_fs")]
    pub fn execute_lsp_task(&mut self, task: LspTask, ctx: &mut ModelContext<Self>) {
        if let LspTask::Spawn { file_path } = &task {
            let has_enabled_server = self
                .root_for_workspace(file_path)
                .and_then(|root| self.enabled_lsp_servers(root))
                .is_some_and(|mut servers| servers.next().is_some());
            if !has_enabled_server {
                return;
            }
        }
        let path_future = LocalShellState::handle(ctx).update(ctx, |shell_state, ctx| {
            shell_state.get_interactive_path_env_var(ctx)
        });
        ctx.spawn(path_future, move |me, path_env_var, ctx| match task {
            LspTask::Install {
                file_path,
                repo_root,
                server_type,
            } => me.handle_install_lsp(file_path, repo_root, server_type, path_env_var, ctx),
            LspTask::Spawn { file_path } => {
                me.handle_spawn_lsp(&file_path, path_env_var, ctx);
            }
        });
    }

    #[cfg(feature = "local_fs")]
    pub fn detect_lsp_workspace_status(
        &mut self,
        repo_root: PathBuf,
        server_type: LSPServerType,
        ctx: &mut ModelContext<Self>,
    ) -> LspRepoStatus {
        self.ensure_workspace(&repo_root, false);
        if self
            .enabled_lsp_servers(&repo_root)
            .is_some_and(|mut servers| servers.any(|server| server == server_type))
        {
            return LspRepoStatus::Enabled;
        }
        match self.lsp_installation_status.get(&server_type).copied() {
            Some(LSPInstallationStatus::Installed) => {
                LspRepoStatus::DisabledAndInstalled { server_type }
            }
            Some(LSPInstallationStatus::NotInstalled) => {
                LspRepoStatus::DisabledAndNotInstalled { server_type }
            }
            Some(LSPInstallationStatus::Checking) => LspRepoStatus::CheckingForInstallation,
            Some(LSPInstallationStatus::Installing) => LspRepoStatus::Installing { server_type },
            None => {
                self.lsp_installation_status
                    .insert(server_type, LSPInstallationStatus::Checking);
                let path_future = LocalShellState::handle(ctx).update(ctx, |shell_state, ctx| {
                    shell_state.get_interactive_path_env_var(ctx)
                });
                let http_client = Arc::new(http_client::Client::new());
                ctx.spawn(
                    async move {
                        let executor = lsp::CommandBuilder::new(path_future.await);
                        server_type
                            .candidate(http_client)
                            .is_installed(&executor)
                            .await
                    },
                    move |me, installed, ctx| {
                        let status = if installed {
                            LSPInstallationStatus::Installed
                        } else {
                            LSPInstallationStatus::NotInstalled
                        };
                        me.lsp_installation_status.insert(server_type, status);
                        ctx.emit(PersistedWorkspaceEvent::InstallStatusUpdate {
                            server_type,
                            status,
                        });
                    },
                );
                LspRepoStatus::CheckingForInstallation
            }
        }
    }
}

#[cfg(test)]
#[path = "persisted_workspace_tests.rs"]
mod tests;
