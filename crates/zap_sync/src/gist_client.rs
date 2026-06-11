//! Gist API client
//!
// author: logic
// date: 2026-05-24

use crate::types::{GistDetail, GistEntry, SyncPlatform};
use reqwest::Client;
use serde_json::json;
use std::time::Duration;
use thiserror::Error;

const GIST_DESCRIPTION: &str = "ZAP_CONFIG";
const GIST_FILENAME: &str = "zap_config.json";
/// Overall HTTP request timeout (including connect + read), to avoid a stalled network leaving the UI stuck on Syncing forever
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// find_gist pagination limit. 100/page, capped at 20 pages = 2000 gists, far beyond any normal user's needs;
/// beyond that, return None early to avoid an infinite loop / triggering a rate limit due to an API pagination quirk
const FIND_GIST_MAX_PAGES: u32 = 20;

/// Gist API client error
#[derive(Debug, Error)]
pub enum GistClientError {
    #[error("network request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Gist not found")]
    NotFound,
    #[error("Token not configured")]
    NoToken,
    #[error("API error: {status} {body}")]
    Api { status: u16, body: String },
}

/// Gist operations trait, supporting both the real client and a test mock
pub trait GistOps: Send + Sync {
    /// Validate whether the Token is valid, returning the username
    fn validate_token(
        &self,
        platform: SyncPlatform,
        token: String,
    ) -> impl std::future::Future<Output = Result<String, GistClientError>> + Send;

    /// Find the Gist whose description is ZAP_CONFIG
    fn find_gist(
        &self,
        platform: SyncPlatform,
        token: String,
    ) -> impl std::future::Future<Output = Result<Option<String>, GistClientError>> + Send;

    /// Create a new Gist
    fn create_gist(
        &self,
        platform: SyncPlatform,
        token: String,
        content: String,
    ) -> impl std::future::Future<Output = Result<String, GistClientError>> + Send;

    /// Update an existing Gist
    fn update_gist(
        &self,
        platform: SyncPlatform,
        token: String,
        gist_id: String,
        content: String,
    ) -> impl std::future::Future<Output = Result<(), GistClientError>> + Send;

    /// Get the Gist file content
    fn get_gist_content(
        &self,
        platform: SyncPlatform,
        token: String,
        gist_id: String,
    ) -> impl std::future::Future<Output = Result<String, GistClientError>> + Send;
}

/// Gist API client, supporting GitHub and Gitee
pub struct GistClient {
    client: Client,
}

impl GistClient {
    /// Create a new GistClient instance.
    /// A build failure is an unrecoverable runtime error (TLS backend init failure, etc.); prefer to panic
    /// rather than silently fall back to a Client::default() without a user-agent — GitHub mandates a UA.
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent("Zap-Terminal")
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .expect("failed to build reqwest client for GistClient");
        Self { client }
    }

    /// Build the auth header; GitHub uses Bearer, Gitee uses the token prefix
    fn auth_header(platform: SyncPlatform, token: &str) -> String {
        match platform {
            SyncPlatform::GitHub => format!("Bearer {token}"),
            SyncPlatform::Gitee => format!("token {token}"),
        }
    }

    /// Validate whether the Token is valid, returning the username
    pub async fn validate_token(
        &self,
        platform: SyncPlatform,
        token: &str,
    ) -> Result<String, GistClientError> {
        if token.is_empty() {
            return Err(GistClientError::NoToken);
        }
        let url = format!("{}/user", platform.base_url());
        let resp = self
            .client
            .get(&url)
            .header("Authorization", Self::auth_header(platform, token))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(GistClientError::Api {
                status: resp.status().as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }

        let user: serde_json::Value = resp.json().await?;
        // A genuine success response must contain a login field; if it doesn't, the response is not
        // the expected GitHub/Gitee /user (it may be an SSO interception page / a proxy faking a 200),
        // and must not be mistaken for successful validation
        let login = user["login"].as_str().ok_or_else(|| GistClientError::Api {
            status: 200,
            body: "response is missing the login field, Token was not actually validated"
                .to_string(),
        })?;
        Ok(login.to_string())
    }

    /// Find the Gist whose description is ZAP_CONFIG, returning its ID
    pub async fn find_gist(
        &self,
        platform: SyncPlatform,
        token: &str,
    ) -> Result<Option<String>, GistClientError> {
        if token.is_empty() {
            return Err(GistClientError::NoToken);
        }
        let base_url = platform.base_url();

        for page in 1..=FIND_GIST_MAX_PAGES {
            let url = format!("{base_url}/gists?page={page}&per_page=100");
            let resp = self
                .client
                .get(&url)
                .header("Authorization", Self::auth_header(platform, token))
                .send()
                .await?;

            if !resp.status().is_success() {
                return Err(GistClientError::Api {
                    status: resp.status().as_u16(),
                    body: resp.text().await.unwrap_or_default(),
                });
            }

            let gists: Vec<GistEntry> = resp.json().await?;

            if gists.is_empty() {
                return Ok(None);
            }

            if let Some(found) = gists
                .iter()
                .find(|g| g.description.as_deref() == Some(GIST_DESCRIPTION))
            {
                return Ok(Some(found.id.clone()));
            }
        }

        // Still not found after MAX_PAGES; treat as nonexistent — the caller will trigger create_gist
        log::warn!(
            "find_gist: still didn't find {GIST_DESCRIPTION} after {FIND_GIST_MAX_PAGES} pages, giving up to avoid an infinite loop / rate limit"
        );
        Ok(None)
    }

    /// Create a new Gist
    pub async fn create_gist(
        &self,
        platform: SyncPlatform,
        token: &str,
        content: &str,
    ) -> Result<String, GistClientError> {
        if token.is_empty() {
            return Err(GistClientError::NoToken);
        }
        let url = format!("{}/gists", platform.base_url());
        let body = json!({
            "description": GIST_DESCRIPTION,
            "public": false,
            "files": {
                GIST_FILENAME: {
                    "content": content
                }
            }
        });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", Self::auth_header(platform, token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(GistClientError::Api {
                status: resp.status().as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }

        let detail: GistDetail = resp.json().await?;
        Ok(detail.id)
    }

    /// Update an existing Gist
    pub async fn update_gist(
        &self,
        platform: SyncPlatform,
        token: &str,
        gist_id: &str,
        content: &str,
    ) -> Result<(), GistClientError> {
        if token.is_empty() {
            return Err(GistClientError::NoToken);
        }
        let url = format!("{}/gists/{gist_id}", platform.base_url());
        let body = json!({
            "description": GIST_DESCRIPTION,
            "files": {
                GIST_FILENAME: {
                    "content": content
                }
            }
        });
        let resp = self
            .client
            .patch(&url)
            .header("Authorization", Self::auth_header(platform, token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(GistClientError::Api {
                status: resp.status().as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }

        Ok(())
    }

    /// Get the Gist file content, handling truncation automatically
    pub async fn get_gist_content(
        &self,
        platform: SyncPlatform,
        token: &str,
        gist_id: &str,
    ) -> Result<String, GistClientError> {
        if token.is_empty() {
            return Err(GistClientError::NoToken);
        }
        let url = format!("{}/gists/{gist_id}", platform.base_url());
        let resp = self
            .client
            .get(&url)
            .header("Authorization", Self::auth_header(platform, token))
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(GistClientError::NotFound);
        }
        if !resp.status().is_success() {
            return Err(GistClientError::Api {
                status: resp.status().as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }

        let detail: serde_json::Value = resp.json().await?;
        let file_obj = &detail["files"][GIST_FILENAME];

        if file_obj["truncated"].as_bool() == Some(true) {
            let raw_url = file_obj["raw_url"]
                .as_str()
                .ok_or(GistClientError::NotFound)?;
            let raw_resp = self
                .client
                .get(raw_url)
                .header("Authorization", Self::auth_header(platform, token))
                .send()
                .await?;
            if !raw_resp.status().is_success() {
                return Err(GistClientError::Api {
                    status: raw_resp.status().as_u16(),
                    body: raw_resp.text().await.unwrap_or_default(),
                });
            }
            Ok(raw_resp.text().await?)
        } else {
            let content = file_obj["content"]
                .as_str()
                .ok_or(GistClientError::NotFound)?;
            Ok(content.to_string())
        }
    }
}

impl GistOps for GistClient {
    async fn validate_token(
        &self,
        platform: SyncPlatform,
        token: String,
    ) -> Result<String, GistClientError> {
        self.validate_token(platform, &token).await
    }

    async fn find_gist(
        &self,
        platform: SyncPlatform,
        token: String,
    ) -> Result<Option<String>, GistClientError> {
        self.find_gist(platform, &token).await
    }

    async fn create_gist(
        &self,
        platform: SyncPlatform,
        token: String,
        content: String,
    ) -> Result<String, GistClientError> {
        self.create_gist(platform, &token, &content).await
    }

    async fn update_gist(
        &self,
        platform: SyncPlatform,
        token: String,
        gist_id: String,
        content: String,
    ) -> Result<(), GistClientError> {
        self.update_gist(platform, &token, &gist_id, &content).await
    }

    async fn get_gist_content(
        &self,
        platform: SyncPlatform,
        token: String,
        gist_id: String,
    ) -> Result<String, GistClientError> {
        self.get_gist_content(platform, &token, &gist_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_header_github() {
        let header = GistClient::auth_header(SyncPlatform::GitHub, "mytoken");
        assert_eq!(header, "Bearer mytoken");
    }

    #[test]
    fn test_auth_header_gitee() {
        let header = GistClient::auth_header(SyncPlatform::Gitee, "mytoken");
        assert_eq!(header, "token mytoken");
    }

    #[tokio::test]
    async fn test_empty_token_returns_no_token_error() {
        // In the test environment the rustls default provider isn't installed; install it first (ignore failure if already installed)
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let client = GistClient::new();
        // validate_token / find_gist / create_gist / update_gist / get_gist_content should return NoToken immediately when the token is empty, without making any HTTP request
        for platform in [SyncPlatform::GitHub, SyncPlatform::Gitee] {
            let r = client.validate_token(platform, "").await;
            assert!(
                matches!(r, Err(GistClientError::NoToken)),
                "validate_token empty token"
            );
            let r = client.find_gist(platform, "").await;
            assert!(
                matches!(r, Err(GistClientError::NoToken)),
                "find_gist empty token"
            );
            let r = client.create_gist(platform, "", "{}").await;
            assert!(
                matches!(r, Err(GistClientError::NoToken)),
                "create_gist empty token"
            );
            let r = client.update_gist(platform, "", "x", "{}").await;
            assert!(
                matches!(r, Err(GistClientError::NoToken)),
                "update_gist empty token"
            );
            let r = client.get_gist_content(platform, "", "x").await;
            assert!(
                matches!(r, Err(GistClientError::NoToken)),
                "get_gist_content empty token"
            );
        }
    }
}
