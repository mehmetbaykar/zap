use std::path::PathBuf;
use std::sync::Arc;

use remote_server::setup::{PreinstallCheckResult, PreinstallStatus};
use remote_server::transport::Error;
use settings::Setting;
use warp_core::SessionId;
use warpui::{Entity, ModelContext, ModelHandle, SingletonEntity, WeakModelHandle};

use super::pty_controller::{EventLoopSender, PtyController};
use crate::remote_server::manager::{RemoteServerManager, RemoteServerManagerEvent};
use crate::remote_server::ssh_transport::SshTransport;
use crate::terminal::model::session::{IsSSHWrapperSession, SessionInfo};
use crate::terminal::model_events::{ModelEvent, ModelEventDispatcher};
use crate::terminal::warpify::settings::{SshExtensionInstallMode, WarpifySettings};

/// Per-SSH-init state machine. Encoding the state as an enum makes invalid
/// transitions unrepresentable and ensures the `SessionInfo` stash cannot be
/// accessed after it has been consumed.
enum SshInitState {
    Idle,
    /// Stash held, `check_binary` in flight.
    AwaitingCheck {
        session_info: SessionInfo,
        transport: SshTransport,
    },
    /// Stash held, choice block showing.
    AwaitingUserChoice {
        session_info: SessionInfo,
        transport: SshTransport,
    },
    /// Stash held, `install_binary` in flight.
    /// `for_update` is `true` when reinstalling over an existing install
    /// (auto-update path) and `false` for a fresh install.
    AwaitingInstall {
        session_id: SessionId,
        session_info: SessionInfo,
        transport: SshTransport,
        #[allow(dead_code)]
        for_update: bool,
    },
    /// Stash held, `connect_session` in flight. Bootstrap is flushed only
    /// once `SessionConnected` arrives (or on connection failure).
    AwaitingConnect {
        session_id: SessionId,
        session_info: SessionInfo,
    },
}

/// Per-pane orchestrator that defers the bootstrap script write for SSH sessions,
/// checks for the remote-server binary, and presents a two-option choice block when the binary is missing.
///
/// Uses a [`WeakModelHandle`] back to [`PtyController`] to avoid preventing
/// `PtyController` from being deallocated.
pub struct RemoteServerController<T: EventLoopSender> {
    pty_controller: WeakModelHandle<PtyController<T>>,
    model_event_dispatcher: ModelHandle<ModelEventDispatcher>,
    state: SshInitState,
}

impl<T: EventLoopSender> Entity for RemoteServerController<T> {
    type Event = ();
}

impl<T: EventLoopSender> RemoteServerController<T> {
    pub fn new(
        pty_controller: WeakModelHandle<PtyController<T>>,
        model_event_dispatcher: ModelHandle<ModelEventDispatcher>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        ctx.subscribe_to_model(&model_event_dispatcher, |me, _, event, ctx| {
            if let ModelEvent::SshInitShell {
                pending_session_info,
            } = event
            {
                me.on_ssh_init_shell_requested(pending_session_info.as_ref().clone(), ctx);
            }
        });

        let mgr = RemoteServerManager::handle(ctx);
        ctx.subscribe_to_model(&mgr, |me, _, event, ctx| match event {
            RemoteServerManagerEvent::BinaryCheckComplete {
                session_id,
                result,
                remote_platform: _,
                preinstall_check,
                has_old_binary,
            } => {
                me.on_binary_check_complete(
                    *session_id,
                    result.clone(),
                    preinstall_check.clone(),
                    *has_old_binary,
                    ctx,
                );
            }
            RemoteServerManagerEvent::BinaryInstallComplete {
                session_id,
                result,
                install_source: _,
            } => {
                me.on_binary_install_complete(*session_id, result.clone(), ctx);
            }
            RemoteServerManagerEvent::SessionConnected { session_id, .. } => {
                me.on_session_connected(*session_id, ctx);
            }
            RemoteServerManagerEvent::SessionConnectionFailed { session_id, .. } => {
                me.on_session_connection_failed(*session_id, ctx);
            }
            RemoteServerManagerEvent::SessionConnecting { .. }
            | RemoteServerManagerEvent::SessionDisconnected { .. }
            | RemoteServerManagerEvent::SessionReconnected { .. }
            | RemoteServerManagerEvent::SessionDeregistered { .. }
            | RemoteServerManagerEvent::HostConnected { .. }
            | RemoteServerManagerEvent::HostDisconnected { .. }
            | RemoteServerManagerEvent::RemoteAgentContextSnapshot { .. }
            | RemoteServerManagerEvent::NavigatedToDirectory { .. }
            | RemoteServerManagerEvent::RepoMetadataSnapshot { .. }
            | RemoteServerManagerEvent::RepoMetadataUpdated { .. }
            | RemoteServerManagerEvent::RepoMetadataDirectoryLoaded { .. }
            | RemoteServerManagerEvent::CodebaseIndexStatusesSnapshot { .. }
            | RemoteServerManagerEvent::CodebaseIndexStatusUpdated { .. }
            | RemoteServerManagerEvent::SetupStateChanged { .. }
            | RemoteServerManagerEvent::BufferUpdated { .. }
            | RemoteServerManagerEvent::BufferConflictDetected { .. }
            | RemoteServerManagerEvent::DiffStateSnapshotReceived { .. }
            | RemoteServerManagerEvent::DiffStateMetadataUpdateReceived { .. }
            | RemoteServerManagerEvent::DiffStateFileDeltaReceived { .. }
            | RemoteServerManagerEvent::GetBranchesResponse { .. }
            | RemoteServerManagerEvent::CommitChainResponse { .. }
            | RemoteServerManagerEvent::GitPushResponse { .. }
            | RemoteServerManagerEvent::CreatePrResponse { .. }
            | RemoteServerManagerEvent::GetPrInfoResponse { .. }
            | RemoteServerManagerEvent::GetCommittedBranchFilesResponse { .. }
            | RemoteServerManagerEvent::GitStatusPushReceived { .. }
            | RemoteServerManagerEvent::GitHubPrInfoPushReceived { .. }
            | RemoteServerManagerEvent::GitHubRepositoryInfoPushReceived { .. } => {}
        });

        Self {
            pty_controller,
            model_event_dispatcher,
            state: SshInitState::Idle,
        }
    }

    /// Extracts the `SessionInfo` from the stash and writes the bootstrap
    /// script to the PTY via `PtyController::initialize_shell`.
    fn flush_stashed_bootstrap(&mut self, session_info: SessionInfo, ctx: &mut ModelContext<Self>) {
        match self.pty_controller.upgrade(ctx) {
            Some(pty) => {
                pty.update(ctx, |pty, ctx| {
                    pty.initialize_shell(&session_info, ctx);
                });
            }
            _ => {
                log::warn!("Remote server PtyController dropped before bootstrap could be flushed");
            }
        }
    }

    /// Idle -> AwaitingCheck
    fn on_ssh_init_shell_requested(&mut self, info: SessionInfo, ctx: &mut ModelContext<Self>) {
        let IsSSHWrapperSession::Yes {
            socket_path,
            external_control_master,
        } = &info.is_ssh_wrapper_session
        else {
            return;
        };
        let session_id = info.session_id;
        let socket_path = socket_path.clone();
        let warp_owns_control_master = !external_control_master;
        debug_assert!(matches!(self.state, SshInitState::Idle));
        match std::mem::replace(&mut self.state, SshInitState::Idle) {
            SshInitState::Idle => {}
            SshInitState::AwaitingCheck {
                session_info: old_info,
                ..
            }
            | SshInitState::AwaitingUserChoice {
                session_info: old_info,
                ..
            }
            | SshInitState::AwaitingInstall {
                session_info: old_info,
                ..
            }
            | SshInitState::AwaitingConnect {
                session_info: old_info,
                ..
            } => {
                self.flush_stashed_bootstrap(old_info, ctx);
            }
        }
        let transport = SshTransport::new(socket_path, warp_owns_control_master);
        self.state = SshInitState::AwaitingCheck {
            session_info: info,
            transport: transport.clone(),
        };
        RemoteServerManager::handle(ctx).update(ctx, |mgr, ctx| {
            mgr.check_binary(session_id, transport, ctx);
        });
    }

    fn on_binary_check_complete(
        &mut self,
        session_id: SessionId,
        result: Result<bool, Arc<Error>>,
        preinstall_check: Option<PreinstallCheckResult>,
        has_old_binary: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingCheck {
            ref session_info, ..
        } = self.state
        else {
            return;
        };
        if session_info.session_id != session_id {
            return;
        }

        let SshInitState::AwaitingCheck {
            session_info,
            transport,
        } = std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingCheck above");
        };
        if let Some(PreinstallCheckResult {
            status: PreinstallStatus::Unsupported { .. },
            ..
        }) = preinstall_check.as_ref()
        {
            self.flush_stashed_bootstrap(session_info, ctx);
            return;
        }

        match result {
            Ok(true) => {
                let socket_path = transport.socket_path().clone();
                let warp_owns_control_master = transport.warp_owns_control_master();
                let connection_label = connection_label_for_session_info(&session_info);
                self.state = SshInitState::AwaitingConnect {
                    session_id,
                    session_info,
                };
                self.connect_session(
                    session_id,
                    socket_path,
                    warp_owns_control_master,
                    connection_label,
                    ctx,
                );
            }
            Ok(false) if has_old_binary => {
                // Auto-update: a prior install exists, so skip the modal
                // and reinstall.
                self.state = SshInitState::AwaitingInstall {
                    session_id,
                    session_info,
                    transport: transport.clone(),
                    for_update: true,
                };
                RemoteServerManager::handle(ctx).update(ctx, |mgr, ctx| {
                    mgr.install_binary(session_id, transport, true, ctx);
                });
            }
            Ok(false) => {
                let install_mode = *WarpifySettings::as_ref(ctx)
                    .ssh_extension_install_mode
                    .value();
                match install_mode {
                    SshExtensionInstallMode::AlwaysAsk => {
                        self.state = SshInitState::AwaitingUserChoice {
                            session_info,
                            transport,
                        };
                        self.model_event_dispatcher.update(ctx, |d, ctx| {
                            d.request_remote_server_block(session_id, ctx);
                        });
                    }
                    SshExtensionInstallMode::AlwaysInstall => {
                        self.state = SshInitState::AwaitingInstall {
                            session_id,
                            session_info,
                            transport: transport.clone(),
                            for_update: false,
                        };
                        RemoteServerManager::handle(ctx).update(ctx, |mgr, ctx| {
                            mgr.install_binary(session_id, transport, false, ctx);
                        });
                    }
                    SshExtensionInstallMode::NeverInstall => {
                        self.flush_stashed_bootstrap(session_info, ctx);
                    }
                }
            }
            Err(err) => {
                log::warn!("Remote server binary check failed: session={session_id:?} error={err}");
                self.flush_stashed_bootstrap(session_info, ctx);
            }
        }
    }

    pub fn handle_ssh_remote_server_install(
        &mut self,
        session_id: SessionId,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingUserChoice { .. } = self.state else {
            log::warn!(
                "Remote server install requested in unexpected state: session={session_id:?}"
            );
            return;
        };

        let SshInitState::AwaitingUserChoice {
            session_info,
            transport,
        } = std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingUserChoice above");
        };

        // Reaching this path implies the user explicitly confirmed a
        // fresh install from the modal. Auto-update flows (with an old
        // binary detected) skip the modal entirely and go through
        // `on_binary_check_complete` with `is_update: true`.
        self.state = SshInitState::AwaitingInstall {
            session_id,
            session_info,
            transport: transport.clone(),
            for_update: false,
        };
        RemoteServerManager::handle(ctx).update(ctx, |mgr, ctx| {
            mgr.install_binary(session_id, transport, false, ctx);
        });
    }

    /// Called when the remote server session is connected. Flushes the
    /// stashed bootstrap so the session initializes with a live client.
    fn on_session_connected(&mut self, session_id: SessionId, ctx: &mut ModelContext<Self>) {
        let SshInitState::AwaitingConnect {
            session_id: expected,
            ..
        } = &self.state
        else {
            return;
        };
        if *expected != session_id {
            return;
        }

        let SshInitState::AwaitingConnect { session_info, .. } =
            std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingConnect above");
        };

        // Flush the stashed bootstrap now that the server is connected.
        // `client_for_session` will return `Some` when the session
        // subsequently initializes, so it picks `RemoteServerCommandExecutor`.
        self.flush_stashed_bootstrap(session_info, ctx);
    }

    /// Called when the remote server connection failed. Flushes the stashed
    /// bootstrap so the SSH session is not permanently blocked.
    fn on_session_connection_failed(
        &mut self,
        session_id: SessionId,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingConnect {
            session_id: expected,
            ..
        } = &self.state
        else {
            return;
        };
        if *expected != session_id {
            return;
        }

        let SshInitState::AwaitingConnect { session_info, .. } =
            std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingConnect above");
        };
        log::warn!("Remote server connection failed: session={session_id:?}");
        self.flush_stashed_bootstrap(session_info, ctx);
    }

    pub fn handle_ssh_remote_server_skip(
        &mut self,
        session_id: SessionId,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingUserChoice { session_info, .. } =
            std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            log::warn!("Remote server skip requested in unexpected state: session={session_id:?}");
            return;
        };
        self.flush_stashed_bootstrap(session_info, ctx);
    }

    fn on_binary_install_complete(
        &mut self,
        session_id: SessionId,
        result: Result<(), Arc<Error>>,
        ctx: &mut ModelContext<Self>,
    ) {
        let expected = match &self.state {
            SshInitState::AwaitingInstall { session_id, .. } => *session_id,
            _ => return,
        };
        if expected != session_id {
            return;
        }

        let (session_info, transport) = match std::mem::replace(&mut self.state, SshInitState::Idle)
        {
            SshInitState::AwaitingInstall {
                session_info,
                transport,
                ..
            } => (session_info, transport),
            _ => unreachable!("just matched AwaitingInstall above"),
        };
        match result {
            Ok(()) => {
                let socket_path = transport.socket_path().clone();
                let warp_owns_control_master = transport.warp_owns_control_master();
                let connection_label = connection_label_for_session_info(&session_info);
                self.state = SshInitState::AwaitingConnect {
                    session_id,
                    session_info,
                };
                self.connect_session(
                    session_id,
                    socket_path,
                    warp_owns_control_master,
                    connection_label,
                    ctx,
                );
            }
            Err(err) => {
                log::warn!(
                    "Remote server binary install failed: session={session_id:?} error={err}"
                );
                self.flush_stashed_bootstrap(session_info, ctx);
            }
        }
    }

    fn connect_session(
        &mut self,
        session_id: SessionId,
        socket_path: PathBuf,
        warp_owns_control_master: bool,
        connection_label: String,
        ctx: &mut ModelContext<Self>,
    ) {
        let transport = SshTransport::new(socket_path, warp_owns_control_master);
        RemoteServerManager::handle(ctx).update(ctx, |mgr, ctx| {
            mgr.connect_session(session_id, transport, Some(connection_label), ctx);
        });
    }
}

fn connection_label_for_session_info(session_info: &SessionInfo) -> String {
    let ssh_host = session_info
        .subshell_info
        .as_ref()
        .and_then(|info| info.ssh_connection_info.as_ref())
        .and_then(|ssh| ssh.host.as_deref());

    connection_label_from_session_hosts(&session_info.user, &session_info.hostname, ssh_host)
}

fn connection_label_from_session_hosts(
    user: &str,
    hostname: &str,
    ssh_host: Option<&str>,
) -> String {
    let host = ssh_host
        .filter(|host| !host.is_empty())
        .map(connection_label_from_ssh_host)
        .or_else(|| (!hostname.is_empty()).then(|| hostname.to_string()));

    connection_label_from_user_and_host(user, host.as_deref())
}

fn connection_label_from_user_and_host(user: &str, host: Option<&str>) -> String {
    match (user.is_empty(), host.filter(|host| !host.is_empty())) {
        (false, Some(host)) => format!("{user}@{host}"),
        (false, None) => user.to_string(),
        (true, Some(host)) => host.to_string(),
        (true, None) => "Remote host".to_string(),
    }
}

fn connection_label_from_ssh_host(host: &str) -> String {
    host.rsplit_once('@')
        .map_or(host, |(_user, host)| host)
        .to_string()
}

#[cfg(test)]
#[path = "remote_server_controller_tests.rs"]
mod tests;
