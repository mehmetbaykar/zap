use std::io::Write;
use std::path::PathBuf;

use anyhow::{bail, Context as _, Result};
use channel_versions::VersionInfo;
use instant::Duration;
use warp_core::channel::{Channel, ChannelState};

use super::release_assets_directory_url;
use super::{DownloadProgress, DownloadReady, ProgressCallback, ReadyForRelaunch};

lazy_static::lazy_static! {
    /// Stores the path to the current executable.
    ///
    /// We cache this before running auto-update because the returned path for
    /// a deleted file includes " (deleted)" _in the file name_, which breaks
    /// the relaunch logic.
    static ref CURRENT_EXE: std::io::Result<PathBuf> = std::env::current_exe();
}

pub(super) async fn download_update_and_cleanup(
    version_info: &VersionInfo,
    _update_id: &str,
    client: &http_client::Client,
    on_progress: ProgressCallback,
) -> Result<DownloadReady> {
    match UpdateMethod::detect() {
        UpdateMethod::Unknown => Ok(DownloadReady::NeedsAuthorization),
        UpdateMethod::AppImage(appimage_path) => {
            appimage::download_update_and_cleanup(version_info, &appimage_path, client, on_progress)
                .await
        }
        UpdateMethod::PackageManager(package_manager) => {
            log::info!("Detected that Zap was installed using {package_manager:?}");
            Ok(DownloadReady::NeedsAuthorization)
        }
    }
}

pub(super) fn apply_update() -> Result<ReadyForRelaunch> {
    // Make sure CURRENT_EXE is initialized before we actually apply the update.
    let _ = CURRENT_EXE.as_ref();

    match UpdateMethod::detect() {
        UpdateMethod::Unknown => bail!("Cannot apply update for unknown update method!"),
        UpdateMethod::AppImage(_) => Ok(ReadyForRelaunch::Yes),
        UpdateMethod::PackageManager(package_manager) => bail!(
            "Zap does not support package-manager autoupdate for {package_manager}; install the new release manually"
        ),
    }
}

pub(super) fn relaunch() -> Result<()> {
    match UpdateMethod::detect() {
        UpdateMethod::Unknown => bail!("Don't know how to relaunch for an unknown update method!"),
        UpdateMethod::AppImage(appimage_path) => appimage::relaunch(&appimage_path),
        UpdateMethod::PackageManager(_) => package_manager::relaunch(),
    }
}

mod appimage {
    use std::path::Path;

    use super::*;

    pub(super) async fn download_update_and_cleanup(
        version_info: &VersionInfo,
        appimage_path: &Path,
        client: &http_client::Client,
        on_progress: ProgressCallback,
    ) -> Result<DownloadReady> {
        use futures::StreamExt as _;
        use instant::Instant;
        const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

        let channel = ChannelState::channel();
        // openWarp: get the real download URL from the GitHub Release cache, bypassing the empty
        // releases_base_url. The official channel still uses release_assets_directory_url.
        let url = if matches!(channel, warp_core::channel::Channel::Oss) {
            // OSS Linux AppImage default asset name "Zap-x86_64.AppImage".
            // The known release asset name is fixed in GitHub Actions.
            let asset = "Zap-x86_64.AppImage";
            if let Some(release) = crate::autoupdate::github::cached_release() {
                if let Some(found) = release.find_asset(asset) {
                    found.browser_download_url.clone()
                } else {
                    log::warn!(
                        "openWarp: cached release tag {} has no asset named {asset}, falling back to the tag URL",
                        release.tag_name
                    );
                    format!(
                        "https://github.com/mehmetbaykar/zap/releases/download/v{}/{asset}",
                        version_info.version
                    )
                }
            } else {
                format!(
                    "https://github.com/mehmetbaykar/zap/releases/download/v{}/{asset}",
                    version_info.version
                )
            }
        } else {
            let Some(appimage_name) = option_env!("APPIMAGE_NAME") else {
                bail!("APPIMAGE_NAME environment variable was not set at compile time!");
            };
            format!(
                "{}/{}",
                release_assets_directory_url(channel, &version_info.version),
                appimage_name
            )
        };

        // Create a temporary file that we'll write the download into.
        let mut new_appimage = tempfile::NamedTempFile::new()?;

        log::info!("Downloading {url} to {}...", new_appimage.path().display());

        let response = client
            .get(&url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await?
            .error_for_status()?;

        // Stream-read chunks + write, throttling progress reports along the way. AppImages are large
        // (tens of MB); a single `.bytes()` would freeze the entire UI until the download finishes,
        // so we use a stream to let the UI see progress.
        let total = response
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        on_progress(DownloadProgress {
            downloaded: 0,
            total,
        });
        let mut downloaded: u64 = 0;
        let mut last_reported = 0u64;
        let mut last_reported_at = Instant::now();
        const REPORT_BYTES_THRESHOLD: u64 = 64 * 1024;
        const REPORT_TIME_THRESHOLD: Duration = Duration::from_millis(250);

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            new_appimage.as_file_mut().write_all(&chunk)?;
            downloaded += chunk.len() as u64;
            if downloaded - last_reported >= REPORT_BYTES_THRESHOLD
                || last_reported_at.elapsed() >= REPORT_TIME_THRESHOLD
            {
                on_progress(DownloadProgress { downloaded, total });
                last_reported = downloaded;
                last_reported_at = Instant::now();
            }
        }
        on_progress(DownloadProgress { downloaded, total });

        // openWarp: verify the SHA-256 of the temp file before overwriting the original AppImage,
        // to defend against CDN man-in-the-middle / network corruption. Other channels skip this
        // (they have their own process).
        if matches!(channel, warp_core::channel::Channel::Oss) {
            let temp_path = new_appimage.path().to_path_buf();
            if let Err(e) =
                crate::autoupdate::verify_oss_asset_sha256(&temp_path, "Zap-x86_64.AppImage")
            {
                // The temp file is cleaned up automatically when NamedTempFile drops, so here we
                // just need to return the error.
                return Err(e);
            }
        }

        log::info!(
            "Copying downloaded AppImage from {} to {}",
            new_appimage.path().display(),
            appimage_path.display()
        );

        // Copy permissions to new app before moving it to ensure we don't leave it
        // in a bad state if the move succeeds but we are unable to update the
        // permissions afterwards.
        new_appimage
            .as_file_mut()
            .set_permissions(appimage_path.metadata()?.permissions())?;

        // Move new AppImage over the one that launched the current Zap instance.
        let new_appimage_path = new_appimage.into_temp_path();
        let mv_status = command::r#async::Command::new("mv")
            .arg(new_appimage_path.as_os_str())
            .arg(appimage_path)
            .output()
            .await?
            .status;
        if !mv_status.success() {
            bail!("Failed to move new AppImage over the old one: {mv_status}");
        }

        // Ensure we don't accidentally drop `new_appimage_path` before we finish
        // moving it to its final location.
        let _ = new_appimage_path;

        Ok(DownloadReady::Yes)
    }

    pub(super) fn relaunch(appimage_path: &Path) -> Result<()> {
        let mut command = command::blocking::Command::new(appimage_path);
        // Pass a flag to the app to let it know it was restarted as part of the
        // autoupdate process.
        command.arg(warp_cli::finish_update_flag());
        // When testing the local channel-versions JSON, let the newly launched binary keep
        // referencing the same file, so we can verify the post-update changelog display.
        if let Ok(path) = std::env::var("WARP_CHANNEL_VERSIONS_PATH") {
            command.env("WARP_CHANNEL_VERSIONS_PATH", path);
        }

        log::info!("Relaunching warp for update...");
        command.spawn()?;
        Ok(())
    }
}

mod package_manager {
    use super::*;

    pub(super) fn relaunch() -> Result<()> {
        let Ok(program) = CURRENT_EXE.as_ref() else {
            bail!(
                "Failed to get path to current executable to relaunch after completing auto-update"
            );
        };
        log::info!("Relaunching using path: {program:?}");
        let mut command = command::blocking::Command::new(program);
        // Add any arguments that were passed to warp, skipping the first
        // argument (the name of the executable) and dropping the flag for
        // finishing an update.
        let finish_update_flag = warp_cli::finish_update_flag();
        command.args(
            std::env::args()
                .skip(1)
                .filter(|arg| arg != &finish_update_flag),
        );
        // Pass a flag to the app to let it know it was restarted as part of the
        // autoupdate process.
        command.arg(finish_update_flag);
        // When testing the local channel-versions JSON, let the newly launched binary keep
        // referencing the same file, so we can verify the post-update changelog display.
        if let Ok(path) = std::env::var("WARP_CHANNEL_VERSIONS_PATH") {
            command.env("WARP_CHANNEL_VERSIONS_PATH", path);
        }

        log::info!("Relaunching warp for update...");
        command.spawn()?;
        Ok(())
    }
}

/// Returns which method should be used to update Zap.
#[derive(Debug)]
pub(crate) enum UpdateMethod {
    /// We don't know how to update Zap.
    Unknown,
    /// Zap is running as an AppImage and should be updated in-place.
    AppImage(PathBuf),
    /// Zap can be updated using the given package manager.
    PackageManager(PackageManager),
}

impl UpdateMethod {
    pub(crate) fn detect() -> Self {
        if let Some(appimage_path) = std::env::var_os("APPIMAGE").map(PathBuf::from) {
            return Self::AppImage(appimage_path);
        }
        if let Ok(package_manager) = PackageManager::detect() {
            // Log the upgrade command the user should run, to make debugging from the logs easier.
            // The UI still falls back to the GitHub release page (the user can download .deb/.rpm
            // and apt install / dnf install themselves).
            package_manager.log_upgrade_hint();
            return Self::PackageManager(package_manager);
        }
        Self::Unknown
    }
}

/// Package managers that we understand and can assist with auto-update
/// for. `Pacman` distinguishes two cases: `PacmanOfficial` means the package comes from the
/// archlinux.org official repository (can run `sudo pacman -Syu` directly), and `PacmanAur` means
/// the package comes from the AUR or a manual local `makepkg -si`, in which case an AUR helper
/// (`paru -Syu` / `yay -Syu`) should be used, and the user should not be told to `pacman -U` a
/// release asset that doesn't exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageManager {
    Apt {
        package_name: String,
    },
    Yum {
        package_name: String,
    },
    Dnf {
        package_name: String,
    },
    Zypper {
        package_name: String,
    },
    /// A pacman package from the archlinux.org official repository (matched by `pacman -Si <pkg>`).
    PacmanOfficial {
        package_name: String,
    },
    /// AUR / manual local install (matched by `pacman -Qi <pkg>` but not by `pacman -Si <pkg>`).
    PacmanAur {
        package_name: String,
    },
}

impl PackageManager {
    /// The candidate package names to query in the system package manager for the current channel,
    /// ordered from most to least likely. For OSS, the deb/rpm/arch bundle scripts all use `zap` as
    /// the package name (see script/linux/bundle_*), but common AUR naming is `zap-bin` / `zap-git`,
    /// so we try a few.
    fn candidate_names(channel: Channel) -> &'static [&'static str] {
        match channel {
            Channel::Stable => &["warp-terminal"],
            Channel::Preview => &["warp-terminal-preview"],
            Channel::Dev => &["warp-terminal-dev"],
            Channel::Integration => &["warp-terminal-integration"],
            Channel::Local => &["warp-terminal-local"],
            // OSS: bundle_deb/rpm/arch all use `zap` as the package name, but an AUR maintainer may
            // choose `zap-bin` / `zap-git`, so try those too.
            Channel::Oss => &["zap", "zap-bin", "zap-git"],
        }
    }

    fn detect() -> Result<Self> {
        let channel = ChannelState::channel();
        let candidates = Self::candidate_names(channel);

        // Try each candidate package name in turn; return the first one any PM recognizes as
        // installed. After a pacman match, use `pacman -Si` to distinguish official repo / AUR.
        for &name in candidates {
            if let Some(pm) = Self::probe_one(name)? {
                return Ok(pm);
            }
        }
        bail!(
            "Could not determine which package manager was used to install \
             this build (tried candidate names: {candidates:?})"
        );
    }

    /// Run the probe script for one specific package name; on a match returns the corresponding
    /// PackageManager, otherwise None. After a pacman match, additionally query `pacman -Si` to
    /// distinguish the official repo from the AUR.
    fn probe_one(package_name: &str) -> Result<Option<Self>> {
        // In the shell script, `$PACKAGE_NAME` is passed in via env, so its content can't be
        // injected via shell escaping (it is passed to the command, not concatenated into an
        // sh -c string).
        let detect_script = r#"
            command -p pacman -Qi "$PACKAGE_NAME" >/dev/null 2>/dev/null
            if [ $? -eq 0 ]; then
              # Distinguish official repo vs AUR/manual. -Si queries the sync database; AUR/manually
              # installed packages won't be synced out.
              if command -p pacman -Si "$PACKAGE_NAME" >/dev/null 2>/dev/null; then
                echo "pacman-official"
              else
                echo "pacman-aur"
              fi
              exit
            fi

            command -p zypper search --match-exact --installed-only "$PACKAGE_NAME" >/dev/null 2>/dev/null
            if [ $? -eq 0 ]; then
              echo "zypper"
              exit
            fi

            command -p dnf list --installed "$PACKAGE_NAME" >/dev/null 2>/dev/null
            if [ $? -eq 0 ]; then
              echo "dnf"
              exit
            fi

            command -p yum list installed "$PACKAGE_NAME" >/dev/null 2>/dev/null
            if [ $? -eq 0 ]; then
              echo "yum"
              exit
            fi

            if [ "$(command -p dpkg-query --show --showformat='${db:Status-Status}' "$PACKAGE_NAME" 2>/dev/null)" = "installed" ]; then
              echo "apt"
              exit
            fi

            exit 1
        "#;

        let output = command::blocking::Command::new("sh")
            .args(["-c", detect_script])
            .env("PACKAGE_NAME", package_name)
            .output();
        let output = match output {
            Ok(o) => o,
            Err(err) => return Err(err).context("Failed to run package manager detection script"),
        };

        // exit 1 = this candidate name wasn't recognized by any PM; not an error, move on to the
        // next candidate.
        if !output.status.success() {
            return Ok(None);
        }
        let stdout = std::str::from_utf8(&output.stdout)
            .map_err(|_| anyhow::anyhow!("non-UTF-8 detect script output"))?;
        let name = package_name.to_string();
        let pm = match stdout.trim() {
            "pacman-official" => Self::PacmanOfficial { package_name: name },
            "pacman-aur" => Self::PacmanAur { package_name: name },
            "zypper" => Self::Zypper { package_name: name },
            "dnf" => Self::Dnf { package_name: name },
            "yum" => Self::Yum { package_name: name },
            "apt" => Self::Apt { package_name: name },
            other => bail!("Unexpected detection output: {other}"),
        };
        Ok(Some(pm))
    }

    /// Write "the upgrade command the user should run" to the log. OSS users can find the precise
    /// instructions in the logs under ~/.local/share/dev.zap.Zap/; the UI still falls back to "go to
    /// GitHub to download" and does not distinguish by package manager.
    fn log_upgrade_hint(&self) {
        let hint = match self {
            Self::Apt { package_name } => {
                format!(
                    "Please run: after downloading the .deb from GitHub Release, `sudo apt install ./{package_name}_*.deb`,\
                     or after adding the release as an apt source, `sudo apt update && sudo apt install {package_name}`"
                )
            }
            Self::Yum { package_name } => {
                format!("Please run: after downloading the .rpm, `sudo yum install ./{package_name}-*.rpm`")
            }
            Self::Dnf { package_name } => {
                format!("Please run: after downloading the .rpm, `sudo dnf install ./{package_name}-*.rpm`")
            }
            Self::Zypper { package_name } => {
                format!("Please run: after downloading the .rpm, `sudo zypper install ./{package_name}-*.rpm`")
            }
            Self::PacmanOfficial { package_name } => {
                format!("Please run: `sudo pacman -Syu {package_name}`")
            }
            Self::PacmanAur { package_name } => {
                format!(
                    "You appear to have installed {package_name} from the AUR. Please upgrade with an AUR helper,\
                     e.g. `paru -Syu {package_name}` or `yay -Syu {package_name}`.\
                     Do not run pacman -U manually; GitHub Release does not include .pkg.tar.zst assets."
                )
            }
        };
        log::info!("openWarp upgrade hint: {hint}");
    }
}

impl std::fmt::Display for PackageManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageManager::Apt { .. } => write!(f, "apt"),
            PackageManager::Yum { .. } => write!(f, "yum"),
            PackageManager::Dnf { .. } => write!(f, "dnf"),
            PackageManager::Zypper { .. } => write!(f, "zypper"),
            PackageManager::PacmanOfficial { .. } => write!(f, "pacman (official)"),
            PackageManager::PacmanAur { .. } => write!(f, "pacman (AUR)"),
        }
    }
}
