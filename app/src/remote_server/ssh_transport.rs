//! SSH-specific implementation of [`RemoteTransport`].
//!
//! [`SshTransport`] uses an existing SSH ControlMaster socket to check/install
//! the remote server binary and to launch the `remote-server-proxy` process
//! whose stdin/stdout become the protocol channel.
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use warpui::r#async::{executor, FutureExt as _};

use remote_server::auth::RemoteServerAuthContext;
use remote_server::client::RemoteServerClient;
use remote_server::manager::RemoteServerExitStatus;
use remote_server::setup::{
    parse_uname_output, remote_server_daemon_dir, PreinstallCheckResult, RemotePlatform,
};
use remote_server::ssh::{ssh_args, SshCommandError};
use remote_server::transport::{Connection, Error, InstallOutcome, InstallSource, RemoteTransport};

/// SSH transport: connects via a ControlMaster socket.
///
/// `socket_path` is the local Unix socket created by the ControlMaster
/// process (`ssh -N -o ControlMaster=yes -o ControlPath=<path>`). All SSH
/// commands (binary check, install, proxy launch) are multiplexed through
/// this socket without re-authenticating.
#[derive(Clone)]
pub struct SshTransport {
    socket_path: PathBuf,
    auth_context: Arc<RemoteServerAuthContext>,
}

impl fmt::Debug for SshTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshTransport")
            .field("socket_path", &self.socket_path)
            .finish_non_exhaustive()
    }
}

impl SshTransport {
    pub fn new(socket_path: PathBuf, auth_context: Arc<RemoteServerAuthContext>) -> Self {
        Self {
            socket_path,
            auth_context,
        }
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    pub fn remote_daemon_socket_path(&self) -> String {
        format!(
            "{}/{}",
            remote_server_daemon_dir(&self.auth_context.remote_server_identity_key()),
            remote_server::setup::daemon_socket_name(),
        )
    }

    pub fn remote_daemon_pid_path(&self) -> String {
        format!(
            "{}/{}",
            remote_server_daemon_dir(&self.auth_context.remote_server_identity_key()),
            remote_server::setup::daemon_pid_name(),
        )
    }

    fn remote_proxy_command(&self) -> String {
        let binary = remote_server::setup::remote_server_binary();
        let identity_key = self.auth_context.remote_server_identity_key();
        let quoted_identity_key = shell_words::quote(&identity_key);
        format!("{binary} remote-server-proxy --identity-key {quoted_identity_key}")
    }
}

/// Runs `uname -sm` on the remote host via the ControlMaster socket and
/// parses the output into a [`RemotePlatform`].
async fn detect_remote_platform(socket_path: &Path) -> Result<RemotePlatform, Error> {
    let output = remote_server::ssh::run_ssh_command(
        socket_path,
        "uname -sm",
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_uname_output(&stdout)
    } else {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Error::Other(anyhow::anyhow!(
            "uname -sm exited with code {code}: {stderr}"
        )))
    }
}

/// Confirms a just-installed binary actually runs at the expected path.
async fn verify_installed_binary(socket_path: &Path) -> Result<(), Error> {
    let output = remote_server::ssh::run_ssh_command(
        socket_path,
        &remote_server::setup::binary_check_command(),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;

    if output.status.success() {
        return Ok(());
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(Error::Other(anyhow!(
        "installed binary check failed with code {code}: {stderr}"
    )))
}

// ===========================================================================
// Zap fork: dev-mode remote-server install path
//
// Upstream / release builds make the remote install script download a
// precompiled remote-server binary from GitHub releases. But in a local source
// build (`cargo run`), this downloads the stale "latest released" binary
// instead of the code the developer just changed, making it impossible to
// debug remote-server changes at all.
//
// Therefore, in a DEBUG source build with no release tag (see
// `remote_server::setup::is_dev_source_build()`), `install_binary()` instead:
//   1. Cross-compiles the `warp` binary locally to x86_64 musl (profile/features
//      exactly matching `script/deploy_remote_server`);
//   2. Uploads the artifact via the existing SSH ControlMaster socket, using
//      `scp_upload`, to the remote path resolved by
//      `remote_server::setup::remote_server_binary()`;
//   3. Skips the GitHub download install script entirely.
//
// If the cross-compile prerequisites are missing (no musl target installed, no
// musl linker), it does not hard-fail; instead it prints a clear warning and
// falls back to the original download install flow, keeping dev usable.
// ===========================================================================

/// musl linker candidates that dev-mode cross-compilation might use (in priority order).
/// On macOS this is usually `x86_64-linux-musl-gcc` (filosottile/musl-cross),
/// on Linux it is commonly `musl-gcc`.
const DEV_MUSL_LINKER_CANDIDATES: &[&str] = &["x86_64-linux-musl-gcc", "musl-gcc"];

/// Returns the current workspace root directory.
///
/// `ssh_transport.rs` belongs to the `app` crate, so `CARGO_MANIFEST_DIR`
/// points at `<workspace>/app`, whose parent directory is the workspace root.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        // In theory `app` always has a parent; on the off chance it doesn't, fall back to the manifest directory itself.
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

/// Returns a PATH with `~/.cargo/bin` (and `$CARGO_HOME/bin`) appended.
///
/// The warp process is often launched by the desktop environment or the system
/// `cargo`, whose PATH may contain only `/usr/bin` and not `~/.cargo/bin`. This
/// causes:
///   - `cargo zigbuild` cannot find the `cargo-zigbuild` subcommand → falls back to musl-gcc;
///   - cargo-zigbuild itself cannot find `cargo` / `rustc`.
/// Cross-compilation subprocesses all use the PATH returned here, ensuring both
/// can be resolved. Returns `None` when no adjustment is needed (no HOME / can't
/// be joined), and the caller keeps the inherited PATH.
fn dev_build_path_env() -> Option<std::ffi::OsString> {
    let mut extra: Vec<PathBuf> = Vec::new();
    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        extra.push(PathBuf::from(cargo_home).join("bin"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        extra.push(PathBuf::from(home).join(".cargo").join("bin"));
    }
    if extra.is_empty() {
        return None;
    }
    let current = std::env::var_os("PATH").unwrap_or_default();
    extra.extend(std::env::split_paths(&current));
    std::env::join_paths(extra).ok()
}

/// Finds the first usable musl linker in `PATH`, returning `None` if none is found.
fn find_musl_linker() -> Option<&'static str> {
    DEV_MUSL_LINKER_CANDIDATES.iter().copied().find(|linker| {
        command::blocking::Command::new(linker)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

/// The build backend used for dev cross-compilation.
enum DevBuildBackend {
    /// `cargo zigbuild`: zig acts as a complete C/C++ musl cross toolchain, so
    /// there's no need to separately install `*-musl-gcc` / `*-musl-g++`, and it
    /// can correctly compile dependencies with C/C++ sources such as
    /// `freetype-sys`. This is the preferred backend.
    Zigbuild,
    /// Native `cargo build` + musl linker. Reliable only when the system has a
    /// complete musl C/C++ cross toolchain — when only `*-musl-gcc` is present
    /// and `*-musl-g++` is missing, C++ dependencies like `freetype-sys` fail
    /// to compile.
    MuslGcc(&'static str),
}

/// Detects whether `cargo-zigbuild` is available.
///
/// Probes `cargo-zigbuild --version` (the binary itself) directly, rather than
/// `cargo zigbuild --version` — the latter fails because the `zigbuild`
/// subcommand parses it as an unknown argument. The probe uses the same PATH as
/// the actual build (injecting `~/.cargo/bin`).
fn cargo_zigbuild_available() -> bool {
    let mut cmd = command::blocking::Command::new("cargo-zigbuild");
    cmd.arg("--version");
    if let Some(path) = dev_build_path_env() {
        cmd.env("PATH", path);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Selects the dev cross-compile backend: prefers `cargo zigbuild`, falls back
/// to native `cargo build` + musl linker. Returns `None` when neither is
/// available, and the caller falls back to download install.
fn select_dev_build_backend() -> Option<DevBuildBackend> {
    if cargo_zigbuild_available() {
        return Some(DevBuildBackend::Zigbuild);
    }
    find_musl_linker().map(DevBuildBackend::MuslGcc)
}

/// Checks whether the `x86_64-unknown-linux-musl` target has been installed via rustup.
async fn musl_target_installed() -> bool {
    let output = command::r#async::Command::new("rustup")
        .arg("target")
        .arg("list")
        .arg("--installed")
        .kill_on_drop(true)
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == remote_server::setup::DEV_MUSL_TARGET),
        // When rustup output can't be obtained, conservatively assume not installed, triggering the fallback.
        _ => false,
    }
}

/// Cross-compiles the local `warp` binary to musl, returning the artifact path.
///
/// profile / features are aligned with `script/deploy_remote_server`.
async fn cross_compile_remote_server(backend: &DevBuildBackend) -> Result<PathBuf> {
    let root = workspace_root();
    // The `[[bin]]` name for the current channel — the OSS fork is `warp-oss` (see app/Cargo.toml).
    // We can't hardcode `warp`: that bin uses `load_config!("local")`, which needs the private
    // `warp-channel-config` to generate `local_config.json`, and the OSS fork fails to compile without it;
    // `warp-oss` (src/bin/oss.rs) inlines `ChannelConfig` and has no such dependency.
    let bin_name = remote_server::setup::binary_name();
    let backend_desc = match backend {
        DevBuildBackend::Zigbuild => "cargo-zigbuild".to_string(),
        DevBuildBackend::MuslGcc(linker) => format!("cargo-build/{linker}"),
    };
    log::info!(
        "dev remote-server: cross-compiling {bin_name} -> {} (profile={}, backend={backend_desc})",
        remote_server::setup::DEV_MUSL_TARGET,
        remote_server::setup::DEV_REMOTE_PROFILE,
    );
    // The first build compiles all of warp, usually taking several minutes.
    // stdout/stderr are inherited directly into the terminal running Zap, so
    // the developer can see cargo's live compilation progress (otherwise it's
    // silent the whole time and easy to mistake for a hang).
    log::info!(
        "dev remote-server: cross-compiling now, the first run usually takes several minutes —— cargo progress will print to\
         the terminal running Zap"
    );

    let status = async {
        let mut cmd = command::r#async::Command::new("cargo");
        cmd.current_dir(&root);
        // Inject `~/.cargo/bin` to ensure `cargo zigbuild` can resolve the
        // `cargo-zigbuild` subcommand, and that cargo-zigbuild can find `cargo` / `rustc`.
        if let Some(path) = dev_build_path_env() {
            cmd.env("PATH", path);
        }
        match backend {
            // zigbuild is a cargo subcommand bundling the zig linker and C/C++ cross compiler,
            // so there's no need to set the LINKER env.
            DevBuildBackend::Zigbuild => {
                cmd.arg("zigbuild");
            }
            // Native cargo build: specify the musl linker via env and override rustflags,
            // to avoid macOS-specific flags in .cargo/config.toml polluting the cross-compile.
            DevBuildBackend::MuslGcc(linker) => {
                cmd.arg("build")
                    .env("CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER", *linker)
                    .env(
                        "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS",
                        "-C symbol-mangling-version=v0",
                    );
            }
        }
        cmd.arg("-p")
            .arg("warp")
            .arg("--bin")
            .arg(bin_name)
            .arg("--target")
            .arg(remote_server::setup::DEV_MUSL_TARGET)
            .arg("--profile")
            .arg(remote_server::setup::DEV_REMOTE_PROFILE)
            .arg("--features")
            .arg(remote_server::setup::DEV_REMOTE_FEATURES)
            // inherit: pass cargo's live progress through to the terminal, instead of buffering silently the whole time.
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .status()
            .await
    }
    .with_timeout(remote_server::setup::DEV_CROSS_COMPILE_TIMEOUT)
    .await
    .map_err(|_| {
        anyhow!(
            "dev remote-server cross-compile timed out (>{:?})",
            remote_server::setup::DEV_CROSS_COMPILE_TIMEOUT
        )
    })?
    .map_err(|e| anyhow!("failed to start cargo build: {e}"))?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(anyhow!(
            "cargo cross-compile failed (exit {code}); see the cargo output in the terminal running Zap"
        ));
    }

    // Artifact location: `<target_dir>/<triple>/<profile>/<bin_name>`.
    // Prefer `CARGO_TARGET_DIR`, otherwise fall back to `<workspace>/target`. The
    // repo doesn't set `[build] target-dir` in `.cargo/config.toml`, so only the env needs to be considered.
    let target_root = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let binary = target_root
        .join(remote_server::setup::DEV_MUSL_TARGET)
        .join(remote_server::setup::DEV_REMOTE_PROFILE)
        .join(bin_name);
    if !binary.is_file() {
        return Err(anyhow!(
            "cross-compile finished but no artifact found at {} (if CARGO_TARGET_DIR is set, please verify the path)",
            binary.display()
        ));
    }
    Ok(binary)
}

/// Dev-mode install: cross-compiles the local `warp` and uploads it to the remote remote-server path.
///
/// The upload target exactly matches `remote_server_binary()`, ensuring the
/// subsequent `check_binary()` / proxy launch can find it. Verification that
/// the upload actually runs is left to `install_binary()`'s unified
/// post-install check, which covers this path along with the download and
/// SCP-fallback paths.
async fn dev_install_local_binary(socket_path: &Path) -> Result<()> {
    // Prerequisite checks: missing any one returns an error, and the caller falls back to download install.
    if !musl_target_installed().await {
        return Err(anyhow!(
            "rust target {} is not installed; run `rustup target add {}`",
            remote_server::setup::DEV_MUSL_TARGET,
            remote_server::setup::DEV_MUSL_TARGET,
        ));
    }
    // Select the cross-compile backend: prefer `cargo zigbuild` (zig bundles a complete C/C++ musl toolchain,
    // able to compile C++ dependencies like freetype-sys), otherwise fall back to musl-gcc. Error if neither is present.
    let backend = select_dev_build_backend().ok_or_else(|| {
        anyhow!(
            "no usable musl cross-compile backend found. Recommend installing cargo-zigbuild + zig\
             (`cargo install cargo-zigbuild`, and install `zig` via your package manager),\
             or install a complete musl C/C++ cross toolchain ({})",
            DEV_MUSL_LINKER_CANDIDATES.join(" / ")
        )
    })?;

    let local_binary = cross_compile_remote_server(&backend).await?;

    // Upload to the exact path resolved by `remote_server_binary()`, creating the parent directory first.
    let remote_binary = remote_server::setup::remote_server_binary();
    let remote_dir = remote_server::setup::remote_server_dir();
    let mkdir_output = remote_server::ssh::run_ssh_command(
        socket_path,
        &format!("mkdir -p {remote_dir}"),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if !mkdir_output.status.success() {
        let code = mkdir_output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&mkdir_output.stderr);
        return Err(anyhow!(
            "remote remote-server directory creation failed (exit {code}): {stderr}"
        ));
    }

    log::info!("dev remote-server: uploading local cross-compiled artifact to {remote_binary} (scp -C compression; hundreds of MB may take several minutes)");
    // The dev artifact is hundreds of MB, so use DEV_UPLOAD_TIMEOUT (far larger than SCP_INSTALL_TIMEOUT),
    // to avoid the large-file upload being interrupted by the 120s timeout and falling back to downloading a stale release.
    remote_server::ssh::scp_upload(
        socket_path,
        &local_binary,
        &remote_binary,
        remote_server::setup::DEV_UPLOAD_TIMEOUT,
    )
    .await?;

    // Grant executable permission.
    let chmod_output = remote_server::ssh::run_ssh_command(
        socket_path,
        &format!("chmod 755 {remote_binary}"),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if !chmod_output.status.success() {
        let code = chmod_output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&chmod_output.stderr);
        return Err(anyhow!("remote chmod failed (exit {code}): {stderr}"));
    }

    Ok(())
}

impl RemoteTransport for SshTransport {
    fn detect_platform(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<RemotePlatform, Error>> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move { detect_remote_platform(&socket_path).await })
    }

    fn run_preinstall_check(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PreinstallCheckResult, Error>> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move {
            match remote_server::ssh::run_ssh_script(
                &socket_path,
                remote_server::setup::PREINSTALL_CHECK_SCRIPT,
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await
            {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    Ok(PreinstallCheckResult::parse(&stdout))
                }
                Ok(output) => {
                    let exit_code = output.status.code().unwrap_or(-1);
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    Err(Error::ScriptFailed { exit_code, stderr })
                }
                Err(e) => Err(e.into()),
            }
        })
    }

    fn check_binary(&self) -> Pin<Box<dyn Future<Output = Result<bool, Error>> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move {
            let cmd = remote_server::setup::binary_check_command();
            log::info!("Running binary check: {cmd}");
            let output = remote_server::ssh::run_ssh_command(
                &socket_path,
                &cmd,
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await?;
            // `<binary> --version` exits 0 when present, executable, and
            // functional. Exit 127 means the binary was not found, and 126
            // means it exists but is not executable. Any other non-zero
            // exit (e.g. SSH exit 255 for a dead connection, or signal
            // termination) is treated as a transport-level failure.
            let code = output.status.code();
            let stdout = String::from_utf8_lossy(&output.stdout);
            log::info!("Binary check result: exit={code:?} stdout={stdout}");
            match code {
                Some(0) => Ok(true),
                Some(126) | Some(127) => Ok(false),
                Some(code) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(Error::Other(anyhow::anyhow!(
                        "binary check exited with code {code}: {stderr}"
                    )))
                }
                None => Err(Error::Other(anyhow::anyhow!(
                    "binary check terminated by signal"
                ))),
            }
        })
    }

    fn check_has_old_binary(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<bool>> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move {
            // Treat the existence of the remote-server install directory
            // itself as evidence of a prior install. If `~/.warp-XX/remote-server`
            // exists, something was installed there before, so any mismatch
            // with the client's expected binary path should be auto-updated
            // rather than surfaced as a first-time install prompt.
            let cmd = format!("test -d {}", remote_server::setup::remote_server_dir());
            let output = remote_server::ssh::run_ssh_command(
                &socket_path,
                &cmd,
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await?;
            // `test -d` exits 0 when present, 1 when missing.
            // Anything else is treated as a check failure.
            match output.status.code() {
                Some(0) => Ok(true),
                Some(1) => Ok(false),
                Some(code) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(anyhow::anyhow!(
                        "remote-server dir check exited with code {code}: {stderr}"
                    ))
                }
                None => Err(anyhow::anyhow!(
                    "remote-server dir check terminated by signal"
                )),
            }
        })
    }

    fn install_binary(&self) -> Pin<Box<dyn Future<Output = InstallOutcome> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move {
            let binary_path = remote_server::setup::remote_server_binary();
            log::info!("Installing remote server binary to {binary_path}");

            // Zap fork: a DEBUG source build (no release tag) takes the dev-mode
            // path, cross-compiling the local `warp` and uploading it, rather
            // than downloading a stale GitHub release. On failure (missing
            // cross-compile prerequisites etc.) it prints a warning and falls
            // back to the regular install flow below, keeping the dev experience
            // intact. Release builds skip this entire block, leaving behavior
            // unchanged.
            if remote_server::setup::is_dev_source_build() {
                log::info!("dev remote-server: detected DEBUG source build, switching to local cross-compile install");
                match dev_install_local_binary(&socket_path).await {
                    Ok(()) => {
                        let mut outcome = InstallOutcome {
                            source: Some(InstallSource::Client),
                            result: Ok(()),
                        };
                        if let Err(error) = verify_installed_binary(&socket_path).await {
                            outcome.result = Err(error);
                        }
                        return outcome;
                    }
                    Err(error) => {
                        log::warn!(
                            "dev remote-server: local cross-compile install unavailable, falling back to download install: {error:#}"
                        );
                        // Fell through; continue to the regular download install flow below.
                    }
                }
            }

            let mut outcome = match install_on_server(&socket_path).await {
                Ok(()) => InstallOutcome {
                    source: Some(InstallSource::Server),
                    result: Ok(()),
                },
                Err(server_err) => {
                    let should_try_scp = !should_skip_scp_fallback(&server_err);

                    if should_try_scp {
                        log::info!("Remote server has no curl/wget, falling back to SCP upload");
                        match scp_install_fallback(&socket_path).await {
                            Ok(()) => InstallOutcome {
                                source: Some(InstallSource::Client),
                                result: Ok(()),
                            },
                            Err(e) => InstallOutcome {
                                source: Some(InstallSource::Client),
                                result: Err(Error::Other(e)),
                            },
                        }
                    } else {
                        InstallOutcome {
                            source: Some(InstallSource::Server),
                            result: Err(server_err),
                        }
                    }
                }
            };

            // Post-install verification: confirm the binary actually
            // landed at the expected path and is functional. This catches
            // silent install failures (e.g. tilde-expansion bugs) that
            // would otherwise surface as a cryptic "Response channel
            // closed" error during the IPC handshake.
            if outcome.result.is_ok() {
                log::info!("Running post-install verification for {binary_path}");
                if let Err(error) = verify_installed_binary(&socket_path).await {
                    outcome.result = Err(error);
                }
            }

            outcome
        })
    }

    fn connect(
        &self,
        executor: Arc<executor::Background>,
    ) -> Pin<Box<dyn Future<Output = Result<Connection>> + Send>> {
        let socket_path = self.socket_path.clone();
        let remote_proxy_command = self.remote_proxy_command();
        Box::pin(async move {
            let mut args = ssh_args(&socket_path);
            args.push(remote_proxy_command);

            // `kill_on_drop(true)` pairs with ownership of the `Child` being
            // returned in the [`Connection`] below: the
            // [`RemoteServerManager`] holds the `Child` on its per-session
            // state, and dropping that state (on explicit teardown or
            // spontaneous disconnect) sends SIGKILL to this ssh process.
            let mut child = command::r#async::Command::new("ssh")
                .args(&args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;

            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to capture child stdin"))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to capture child stdout"))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to capture child stderr"))?;

            let (client, event_rx, failure_rx, stderr_tail) =
                RemoteServerClient::from_child_streams(stdin, stdout, stderr, &executor);
            Ok(Connection {
                client,
                event_rx,
                failure_rx,
                child,
                control_path: Some(socket_path),
                stderr_tail,
            })
        })
    }

    fn remove_remote_server_binary(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move {
            let cmd = format!("rm -f {}", remote_server::setup::remote_server_binary());
            log::info!("Removing stale remote server binary: {cmd}");
            let output = remote_server::ssh::run_ssh_command(
                &socket_path,
                &cmd,
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await?;
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(anyhow::anyhow!("Failed to remove binary: {stderr}"))
            }
        })
    }

    /// SSH exit code 255 indicates a connection-level error (broken pipe,
    /// connection reset, host unreachable) — the ControlMaster's TCP
    /// connection is dead. A signal kill also suggests the transport was
    /// torn down. In either case, reconnecting through the same
    /// ControlMaster is futile.
    fn is_reconnectable(&self, exit_status: Option<&RemoteServerExitStatus>) -> bool {
        match exit_status {
            Some(s) => s.code != Some(255) && !s.signal_killed,
            // No exit status available — optimistically allow reconnect.
            None => true,
        }
    }
}

/// Exit codes where SCP fallback would not help because the failure
/// is on the remote host itself (not a network/download issue).
fn should_skip_scp_fallback(error: &Error) -> bool {
    // Unsupported arch/OS — SCP won't change the architecture
    matches!(error, Error::ScriptFailed { exit_code , .. } if *exit_code == 2)
}

/// Runs the install script on the remote host to download and install
/// the binary directly from the CDN.
async fn install_on_server(socket_path: &Path) -> Result<(), Error> {
    let script = remote_server::setup::install_script(None);
    match remote_server::ssh::run_ssh_script(
        socket_path,
        &script,
        remote_server::setup::INSTALL_TIMEOUT,
    )
    .await
    {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Err(Error::ScriptFailed { exit_code, stderr })
        }
        Err(SshCommandError::TimedOut { .. }) => Err(Error::TimedOut),
        Err(e) => Err(Error::Other(e.into())),
    }
}

/// SCP install fallback: downloads the tarball locally, uploads it to
/// the remote via SCP, then re-invokes the install script with the
/// staging path baked in so the shared extraction tail runs.
async fn scp_install_fallback(socket_path: &Path) -> anyhow::Result<()> {
    use std::process::Stdio;

    // Detect the remote platform so we can construct the correct download URL.
    // This is a redundant uname call (the manager already ran detect_platform
    // earlier), but it only happens on the rare SCP fallback path and avoids
    // threading the platform through the trait.
    let platform = detect_remote_platform(socket_path)
        .await
        .map_err(|e| anyhow::anyhow!("SCP fallback: {e:#}"))?;

    let url = remote_server::setup::download_tarball_url(&platform);
    let remote_tarball_path = format!(
        "{}/zap-upload.tar.gz",
        remote_server::setup::remote_server_dir()
    );
    let timeout = remote_server::setup::SCP_INSTALL_TIMEOUT;

    // 1. Download the tarball locally into a temp directory.
    let tmp_dir =
        tempfile::tempdir().map_err(|e| anyhow::anyhow!("Failed to create local temp dir: {e}"))?;
    let temp_client_tarball_path = tmp_dir.path().join("zap.tar.gz");

    log::info!("Downloading tarball locally from {url}");
    let output = command::r#async::Command::new("curl")
        // -f: fail silently on HTTP errors (non-zero exit instead of HTML error page)
        // -S: show errors even when -f is used
        // -L: follow redirects (the CDN may 302 to a regional edge)
        .arg("-fSL")
        .arg("--connect-timeout")
        .arg("15")
        .arg(&url)
        .arg("-o")
        .arg(&temp_client_tarball_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn local curl: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "Local curl failed (exit {:?}): {stderr}",
            output.status.code()
        ));
    }

    // 2. Upload to the remote via SCP.
    log::info!("Uploading tarball to remote at {remote_tarball_path}");
    remote_server::ssh::scp_upload(
        socket_path,
        &temp_client_tarball_path,
        &remote_tarball_path,
        timeout,
    )
    .await?;

    // 3. Run the install script with the staging path baked in.
    //    The script's `staging_tarball_path` variable is non-empty, so it
    //    skips the download and extracts from the uploaded tarball.
    log::info!("Running extraction via install script with tarball at {remote_tarball_path}");

    let script = remote_server::setup::install_script(Some(&remote_tarball_path));

    let output = remote_server::ssh::run_ssh_script(socket_path, &script, timeout).await?;
    if output.status.success() {
        Ok(())
    } else {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow::anyhow!(
            "Extraction script failed (exit {code}): {stderr}"
        ))
    }
}

#[cfg(test)]
#[path = "ssh_transport_tests.rs"]
mod tests;
