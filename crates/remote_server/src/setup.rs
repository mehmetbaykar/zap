mod glibc;

use std::time::Duration;

use anyhow::anyhow;
pub use glibc::{GlibcVersion, RemoteLibc};
use warp_core::channel::{Channel, ChannelState};
pub const REMOTE_SERVER_ARTIFACT_VERSION_UNPINNED: &str = "unversioned";

/// State machine for the remote server install → launch → initialize flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteServerSetupState {
    /// Checking if the binary exists on remote.
    Checking,
    /// Downloading and installing the binary for the first time on this host.
    Installing { progress_percent: Option<u8> },
    /// Replacing an existing install with a differently-versioned binary.
    /// Rendered as "Updating..." in the UI so the user understands this
    /// isn't a fresh install.
    Updating,
    /// Binary is launched, waiting for InitializeResponse.
    Initializing,
    /// Handshake complete. Ready.
    Ready,
    /// Something failed. Fall back to ControlMaster.
    Failed { error: String },
    /// Preinstall check classified the host as incompatible with the
    /// prebuilt remote-server binary. The controller treats this as a
    /// clean fall-back to the legacy ControlMaster-backed SSH flow,
    /// distinct from `Failed` (which is rendered as a real error).
    Unsupported { reason: UnsupportedReason },
}

impl RemoteServerSetupState {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported { .. })
    }

    pub fn is_terminal(&self) -> bool {
        self.is_ready() || self.is_failed() || self.is_unsupported()
    }

    pub fn is_in_progress(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Installing { .. } | Self::Updating | Self::Initializing
        )
    }

    pub fn is_connecting(&self) -> bool {
        matches!(
            self,
            Self::Installing { .. } | Self::Updating | Self::Initializing
        )
    }
}

impl From<&crate::transport::Error> for RemoteServerSetupState {
    fn from(error: &crate::transport::Error) -> Self {
        if let Some(reason) = UnsupportedReason::from_transport_error(error) {
            Self::Unsupported { reason }
        } else {
            Self::Failed {
                error: error.to_string(),
            }
        }
    }
}

/// Outcome of [`crate::transport::RemoteTransport::run_preinstall_check`].
///
/// The script runs over the existing SSH socket before any install UI
/// surfaces and reports whether the host can run the prebuilt
/// remote-server binary. The Rust side is intentionally a thin parser
/// over the script's structured stdout (see `preinstall_check.sh`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreinstallCheckResult {
    pub status: PreinstallStatus,
    pub libc: RemoteLibc,
    /// Verbatim, trimmed script stdout for diagnosing `Unknown` outcomes on
    /// exotic distros.
    pub raw: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreinstallStatus {
    Supported,
    Unsupported {
        reason: UnsupportedReason,
    },
    /// Probe ran but couldn't classify the host. Treated as supported
    /// (fail open) by [`PreinstallCheckResult::is_supported`] so we keep
    /// today's install-and-try behavior on hosts where the probe is
    /// unreliable.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnsupportedReason {
    GlibcTooOld {
        detected: GlibcVersion,
        required: GlibcVersion,
    },
    NonGlibc {
        name: String,
    },
    UnsupportedOs {
        os: String,
    },
    UnsupportedArch {
        arch: String,
    },
}

impl UnsupportedReason {
    pub fn from_transport_error(error: &crate::transport::Error) -> Option<Self> {
        match error {
            crate::transport::Error::UnsupportedOs { os } => {
                Some(Self::UnsupportedOs { os: os.clone() })
            }
            crate::transport::Error::UnsupportedArch { arch } => {
                Some(Self::UnsupportedArch { arch: arch.clone() })
            }
            crate::transport::Error::TimedOut
            | crate::transport::Error::ScriptFailed { .. }
            | crate::transport::Error::Other(_) => None,
        }
    }
}

impl PreinstallCheckResult {
    pub fn unsupported(reason: UnsupportedReason) -> Self {
        Self {
            status: PreinstallStatus::Unsupported { reason },
            libc: RemoteLibc::Unknown,
            raw: String::new(),
        }
    }
    /// Whether the host is supported. Both `Supported` and `Unknown`
    /// return true — only positive detection of an incompatible libc
    /// triggers the silent fall-back.
    pub fn is_supported(&self) -> bool {
        match self.status {
            PreinstallStatus::Supported | PreinstallStatus::Unknown => true,
            PreinstallStatus::Unsupported { .. } => false,
        }
    }

    /// Parses the structured `key=value` stdout emitted by
    /// `preinstall_check.sh`. Tolerates unknown keys and lines without
    /// `=` (forward-compatibility): future versions of the script can
    /// add new keys without coordinating a client release.
    pub fn parse(stdout: &str) -> Self {
        let mut status_str: Option<&str> = None;
        let mut reason_str: Option<&str> = None;
        let mut libc_family: Option<&str> = None;
        let mut libc_version: Option<&str> = None;
        let mut required_glibc: Option<&str> = None;

        for line in stdout.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "status" => status_str = Some(value.trim()),
                "reason" => reason_str = Some(value.trim()),
                "libc_family" => libc_family = Some(value.trim()),
                "libc_version" => libc_version = Some(value.trim()),
                "required_glibc" => required_glibc = Some(value.trim()),
                _ => {} // ignore unknown keys
            }
        }

        let libc = glibc::parse_libc(libc_family, libc_version);
        let status = parse_status(status_str, reason_str, &libc, required_glibc);

        Self {
            status,
            libc,
            raw: stdout.trim().to_string(),
        }
    }
}

fn parse_status(
    status: Option<&str>,
    reason: Option<&str>,
    _libc: &RemoteLibc,
    _required_glibc: Option<&str>,
) -> PreinstallStatus {
    // remote-server is now a static musl binary (see the comment at the top
    // of `preinstall_check.sh`) and does not link the host's dynamic libc.
    // Therefore `glibc_too_old` / `non_glibc` are no longer reasons to be
    // "unsupported" —— any glibc version and musl/uclibc hosts can run the
    // binary. The new script no longer emits these two reasons; but an old
    // remote may still have the old script cached, so here we treat these
    // libc gate reasons as `Supported` rather than `Unsupported`, keeping the
    // judgment consistent between the new and old scripts.
    match status {
        Some("supported") => PreinstallStatus::Supported,
        Some("unsupported") => match reason {
            // libc gate reasons left over from the old script: obsolete for a
            // static binary, treated as supported.
            Some("glibc_too_old") | Some("non_glibc") => PreinstallStatus::Supported,
            // Other unrecognized unsupported reasons: fail open to be safe.
            _ => PreinstallStatus::Unknown,
        },
        // status=unknown, missing, or anything else → fail open.
        _ => PreinstallStatus::Unknown,
    }
}

/// The bundled preinstall check script. Loaded as a string so the SSH
/// transport can pipe it through the existing ControlMaster socket via
/// [`crate::ssh::run_ssh_script`].
///
/// The script is intentionally self-contained — the supported-glibc
/// floor is hardcoded inside the script (see `preinstall_check.sh`)
/// rather than templated from Rust.
pub const PREINSTALL_CHECK_SCRIPT: &str = include_str!("preinstall_check.sh");

/// Detected remote platform from `uname -sm` output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePlatform {
    pub os: RemoteOs,
    pub arch: RemoteArch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOs {
    Linux,
    MacOs,
}

impl RemoteOs {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::MacOs => "macos",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteArch {
    X86_64,
    Aarch64,
}

impl RemoteArch {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }
}

/// Parse `uname -sm` output into a `RemotePlatform`.
///
/// The expected format is `<os> <arch>`, e.g. `Linux x86_64` or `Darwin arm64`.
/// Takes the last line to skip any shell initialization output.
pub fn parse_uname_output(
    output: &str,
) -> std::result::Result<RemotePlatform, crate::transport::Error> {
    use crate::transport::Error;

    let line = output
        .lines()
        .last()
        .ok_or_else(|| Error::Other(anyhow!("empty uname output")))
        .map(str::trim)?;

    let mut parts = line.split_whitespace();
    let os_str = parts
        .next()
        .ok_or_else(|| Error::Other(anyhow!("missing OS in uname output: {line}")))?;
    let arch_str = parts
        .next()
        .ok_or_else(|| Error::Other(anyhow!("missing arch in uname output: {line}")))?;

    let os = match os_str {
        "Linux" => RemoteOs::Linux,
        "Darwin" => RemoteOs::MacOs,
        other => {
            return Err(Error::UnsupportedOs {
                os: other.to_string(),
            })
        }
    };

    let arch = match arch_str {
        "x86_64" | "amd64" => RemoteArch::X86_64,
        "aarch64" | "arm64" => RemoteArch::Aarch64,
        other => {
            return Err(Error::UnsupportedArch {
                arch: other.to_string(),
            })
        }
    };

    Ok(RemotePlatform { os, arch })
}

/// Returns the remote binary install directory, isolated per channel.
///
/// - stable:      `~/.warp/remote-server`
/// - preview:     `~/.warp-preview/remote-server`
/// - dev:         `~/.warp-dev/remote-server`
/// - local:       `~/.warp-local/remote-server`
/// - integration: `~/.warp-dev/remote-server`
/// - warp-oss:    `~/.zap/remote-server`
pub fn remote_server_dir() -> String {
    let warp_dir = match ChannelState::channel() {
        Channel::Stable => ".warp",
        Channel::Preview => ".warp-preview",
        Channel::Dev | Channel::Integration => ".warp-dev",
        Channel::Local => ".warp-local",
        Channel::Oss => ".zap",
    };
    format!("~/{warp_dir}/remote-server")
}

/// Returns the remote OS user's directory used for the daemon socket and PID file.
pub fn remote_server_daemon_dir() -> String {
    format!("{}/daemon", remote_server_dir())
}

/// Returns the remote OS user's directory used for daemon-owned local data files.
pub fn remote_server_daemon_data_dir() -> String {
    format!("{}/data", remote_server_dir())
}

/// Returns a short, deterministic 8-hex-char hash of the app version string.
///
/// Used to version-discriminate daemon socket and PID files without
/// embedding the full version string in the filename, which would push
/// the Unix domain socket path over the `sun_path` limit (107 bytes on
/// Linux, 103 on macOS) for users with long home directory paths.
pub fn version_hash() -> Option<String> {
    use std::hash::{Hash, Hasher};

    let version = ChannelState::app_version()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    version.hash(&mut hasher);
    Some(format!("{:016x}", hasher.finish())[..8].to_string())
}

/// Returns the daemon socket filename, versioned with a short hash when
/// a release tag is baked in.
///
/// - With `GIT_RELEASE_TAG`:    `server-{hash8}.sock`  (e.g. `server-a1b2c3d4.sock`)
/// - Without (plain cargo run): `server.sock`
pub fn daemon_socket_name() -> String {
    match version_hash() {
        Some(hash) => format!("server-{hash}.sock"),
        None => "server.sock".to_string(),
    }
}

/// Returns the daemon PID filename, versioned with a short hash when a
/// release tag is baked in.
///
/// - With `GIT_RELEASE_TAG`:    `server-{hash8}.pid`
/// - Without (plain cargo run): `server.pid`
pub fn daemon_pid_name() -> String {
    match version_hash() {
        Some(hash) => format!("server-{hash}.pid"),
        None => "server.pid".to_string(),
    }
}

/// Returns the remote remote-server binary file name.
pub fn binary_name() -> &'static str {
    ChannelState::channel().cli_command_name()
}

/// Returns the full remote binary path for the current channel and client
/// version.
///
/// Local builds keep an unversioned path so that `script/deploy_remote_server`
/// can overwrite the same development slot. Zap release builds carrying a
/// `GIT_RELEASE_TAG` use a version suffix, so a new version naturally triggers
/// a reinstall; local source builds have no release tag and still use the
/// unsuffixed path.
pub fn remote_server_binary() -> String {
    let dir = remote_server_dir();
    let name = binary_name();
    match ChannelState::channel() {
        Channel::Local => format!("{dir}/{name}"),
        Channel::Oss if ChannelState::app_version().is_none() => format!("{dir}/{name}"),
        Channel::Oss => format!("{dir}/{name}-{}", pinned_version()),
        Channel::Stable | Channel::Preview | Channel::Dev | Channel::Integration => {
            format!("{dir}/{name}-{}", pinned_version())
        }
    }
}

/// Returns the shell command to verify the remote server binary is
/// installed and functional by running it with `--version`.
///
/// Exits 0 when the binary is present, executable, and can parse its
/// own arguments. A missing binary produces exit 127 (command not
/// found) or 126 (not executable), and a corrupted binary will fail
/// with a non-zero exit of its own.
pub fn binary_check_command() -> String {
    format!("{} --version", remote_server_binary())
}

/// Returns the shell command to remove the current remote-server binary.
///
/// The global bundled resources directory is deliberately left in place:
/// the next install overwrites it, and an older daemon that is still
/// running parsed its skills at startup.
pub fn remote_server_removal_command() -> String {
    format!("rm -f {}", remote_server_binary())
}

/// Returns the version number used for versioned install paths. Prefers the
/// compile-time-injected `GIT_RELEASE_TAG`; when there is no release tag it
/// falls back to `CARGO_PKG_VERSION`, keeping channels that need versioned
/// paths deterministic and failing clearly when the corresponding release
/// asset is missing, rather than mistakenly using an unversioned path.
fn pinned_version() -> &'static str {
    ChannelState::app_version().unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// Returns the version key used to identify remote-server download artifacts.
///
/// This must match the versioning used by [`download_tarball_url`] and
/// [`install_script`], so versioned download URLs do not reuse stale tarballs
/// from a previous client version.
pub fn remote_server_artifact_version() -> &'static str {
    match ChannelState::channel() {
        Channel::Local | Channel::Oss => REMOTE_SERVER_ARTIFACT_VERSION_UNPINNED,
        Channel::Stable | Channel::Preview | Channel::Dev | Channel::Integration => {
            pinned_version()
        }
    }
}

/// Name of the global, version-independent resources directory inside
/// [`remote_server_dir`], populated by the install script from the
/// artifact's `resources/` tree (bundled skills, settings schema).
pub const BUNDLED_RESOURCES_DIR_NAME: &str = "bundled_resources";

/// Returns the global, version-independent directory where the install
/// script places the artifact's `resources/` tree. Shell-form path
/// (`~/...`); the daemon expands it against its own home directory.
///
/// Deliberately not version-scoped: the last install wins, and slight
/// version skew between the resources and a running daemon is accepted
/// (the daemon parses its skills once at startup).
pub fn remote_server_bundled_resources_dir() -> String {
    format!("{}/{}", remote_server_dir(), BUNDLED_RESOURCES_DIR_NAME)
}

/// The install script template, loaded from a standalone `.sh` file for
/// readability. Placeholders like `{download_base_url}` are substituted by
/// [`install_script`].
const INSTALL_SCRIPT_TEMPLATE: &str = include_str!("install_remote_server.sh");

/// Returns the install script. When `staging_tarball_path` is non-empty, the
/// script skips the remote download and instead extracts the tarball the
/// client pre-uploaded via SCP.
pub fn install_script(staging_tarball_path: Option<&str>) -> String {
    let version_suffix = version_suffix();
    INSTALL_SCRIPT_TEMPLATE
        .replace("{download_base_url}", &download_url())
        .replace("{install_dir}", &remote_server_dir())
        .replace("{binary_name}", binary_name())
        .replace("{version_suffix}", &version_suffix)
        .replace("{bundled_resources_dir_name}", BUNDLED_RESOURCES_DIR_NAME)
        .replace(
            "{no_http_client_exit_code}",
            &NO_HTTP_CLIENT_EXIT_CODE.to_string(),
        )
        .replace("{staging_tarball_path}", staging_tarball_path.unwrap_or(""))
}

/// Builds the base URL for downloading Zap CLI release assets.
fn download_url() -> String {
    let release_path = match ChannelState::app_version() {
        Some(tag) => format!("download/{tag}"),
        None => "latest/download".to_string(),
    };
    format!("https://github.com/mehmetbaykar/zap/releases/{release_path}")
}

fn version_suffix() -> String {
    match ChannelState::channel() {
        Channel::Local => String::new(),
        Channel::Oss if ChannelState::app_version().is_none() => String::new(),
        Channel::Oss | Channel::Stable | Channel::Preview | Channel::Dev | Channel::Integration => {
            format!("-{}", pinned_version())
        }
    }
}

/// Returns the Zap CLI tarball URL for the given remote platform.
pub fn download_tarball_url(platform: &RemotePlatform) -> String {
    format!(
        "{}/zap-{}-{}.tar.gz",
        download_url(),
        platform.os.as_str(),
        platform.arch.as_str(),
    )
}

/// Zap fork: in development mode (DEBUG source build, no release tag), the
/// SSH transport no longer downloads a stale release from GitHub, but instead
/// cross-compiles the current `warp` binary locally and uploads it. The
/// constants below centrally describe that cross-compilation artifact, kept
/// consistent with `script/deploy_remote_server` (same profile / same
/// features / same target) to avoid the two diverging.
///
/// Cross-compilation target triple.
pub const DEV_MUSL_TARGET: &str = "x86_64-unknown-linux-musl";

/// The cargo profile used for cross-compilation. Corresponds to
/// `[profile.dev-remote]` in `Cargo.toml`, which inherits `dev` and strips
/// symbols to reduce size and speed up uploads.
pub const DEV_REMOTE_PROFILE: &str = "dev-remote";

/// The features enabled for cross-compilation, consistent with
/// `script/deploy_remote_server`.
pub const DEV_REMOTE_FEATURES: &str = "release_bundle,crash_reporting,standalone,agent_mode_debug";

/// Determines whether we are currently on the "development-mode remote-server
/// install" path.
///
/// Default condition: a DEBUG build (`debug_assertions`) with no injected
/// `GIT_RELEASE_TAG` (`app_version().is_none()`, i.e. a local source build,
/// not a release). This uses the same standard for "no release tag" as
/// `remote_server_binary()` / `download_url()`. Release builds are always
/// `false`, with completely unchanged behavior.
///
/// Explicit override: setting `WARP_REMOTE_SERVER_FROM_LOCAL=1` forces the
/// local cross-compilation path (`0` / unset counts as off). Used to
/// temporarily test a local remote-server within a release build.
pub fn is_dev_source_build() -> bool {
    if let Some(raw) = std::env::var_os("WARP_REMOTE_SERVER_FROM_LOCAL") {
        let lossy = raw.to_string_lossy();
        let trimmed = lossy.trim();
        let disabled =
            trimmed.is_empty() || trimmed == "0" || trimmed.eq_ignore_ascii_case("false");
        if !disabled {
            return true;
        }
    }
    cfg!(debug_assertions) && ChannelState::app_version().is_none()
}

/// Timeout for checking whether the binary exists.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for the install script (curl/wget path).
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);

/// Exit code emitted by the install script when neither curl nor wget exists.
pub const NO_HTTP_CLIENT_EXIT_CODE: i32 = 3;

/// Timeout for the SCP upload fallback path (local download + SCP +
/// extraction). Higher than [`INSTALL_TIMEOUT`] because SCP transfers the
/// tarball over the user's SSH link, which is typically slower than the
/// remote host's direct internet connection.
pub const SCP_INSTALL_TIMEOUT: Duration = Duration::from_secs(240);

/// Development-mode cross-compilation may have to compile the entire crate
/// graph from scratch, so it gets a very generous timeout.
pub const DEV_CROSS_COMPILE_TIMEOUT: Duration = Duration::from_secs(900);

/// Timeout for uploading the locally cross-compiled artifact in development
/// mode. The dev binary (unoptimized + debug info) is hundreds of MB, and
/// even with scp's `-C` compression, uploading over the public internet can
/// take several minutes, so it gets a generous limit well beyond
/// `SCP_INSTALL_TIMEOUT`.
pub const DEV_UPLOAD_TIMEOUT: Duration = Duration::from_secs(1800);

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
