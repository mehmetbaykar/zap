// On the Oss channel, autoupdate goes through the GitHub Releases API rather
// than Zap's official channel_versions / GCS feed. This module is only
// responsible for "fetching the latest release metadata" + "picking assets by
// file name"; the actual download-to-disk + opening the directory is handled
// by windows.rs / mac.rs.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context as _, Result};
use lazy_static::lazy_static;
use serde::Deserialize;

const REPO_OWNER: &str = "mehmetbaykar";
const REPO_NAME: &str = "zap";

// GitHub requires a User-Agent; we also declare the API version explicitly to
// avoid drift if the default changes in the future.
const USER_AGENT: &str = "Zap-Autoupdate";
const ACCEPT: &str = "application/vnd.github+json";
const API_VERSION: &str = "2022-11-28";

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    pub html_url: String,
    pub assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    /// Asset digest returned by the GitHub Releases API (2024.12+) in the asset
    /// metadata, formatted as `"sha256:<hex>"`. None for older releases that do
    /// not include this field.
    #[serde(default)]
    pub digest: Option<String>,
}

impl GithubAsset {
    /// Parse the `digest` field and return the lowercase hex SHA-256 (64 chars),
    /// or None. GitHub currently only returns the sha256 algorithm; any other
    /// algorithm is treated as None so the caller skips verification rather than
    /// granting a "green pass" based on an unknown algorithm.
    pub fn sha256_hex(&self) -> Option<String> {
        let raw = self.digest.as_ref()?;
        let hex = raw.strip_prefix("sha256:")?;
        if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            Some(hex.to_ascii_lowercase())
        } else {
            None
        }
    }
}

impl GithubRelease {
    pub fn version(&self) -> &str {
        self.tag_name.trim_start_matches('v')
    }

    pub fn find_asset(&self, expected_name: &str) -> Option<&GithubAsset> {
        self.assets.iter().find(|a| a.name == expected_name)
    }
}

lazy_static! {
    /// The most recently fetched release. Written by fetch_version, read by
    /// download_update. This means the download stage does not have to call the
    /// GitHub API again, and avoids a race (the release being refreshed between
    /// the two requests).
    static ref LATEST_RELEASE: Mutex<Option<GithubRelease>> = Mutex::new(None);
}

pub fn cached_release() -> Option<GithubRelease> {
    LATEST_RELEASE.lock().ok().and_then(|g| g.clone())
}

fn store_cached(release: GithubRelease) {
    if let Ok(mut guard) = LATEST_RELEASE.lock() {
        *guard = Some(release);
    }
}

pub async fn fetch_latest_release(client: &http_client::Client) -> Result<GithubRelease> {
    let url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest");
    log::info!("Fetching latest release from {url}");
    let release: GithubRelease = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", ACCEPT)
        .header("X-GitHub-Api-Version", API_VERSION)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .context("failed to call the GitHub Releases API")?
        .error_for_status()
        .context("GitHub Releases API returned a non-2xx status code")?
        .json()
        .await
        .context("failed to parse the GitHub Releases JSON")?;
    log::info!(
        "GitHub latest release: tag={} assets={}",
        release.tag_name,
        release.assets.len()
    );
    store_cached(release.clone());
    Ok(release)
}
