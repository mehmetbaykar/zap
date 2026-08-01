use super::*;

fn make_manager(keys: ApiKeys) -> ApiKeyManager {
    make_manager_with_grok(keys, None)
}

fn make_manager_with_grok(keys: ApiKeys, grok_tokens: Option<GrokTokens>) -> ApiKeyManager {
    ApiKeyManager {
        keys,
        grok_tokens,
        #[cfg(not(target_family = "wasm"))]
        grok_refresh_allowed: false,
        #[cfg(not(target_family = "wasm"))]
        grok_refresh_waiters: None,
        aws_credentials_state: AwsCredentialsState::Missing,
        secure_storage_write_version: 0,
        grok_secure_storage_write_version: 0,
    }
}

fn grok_tokens(access_token: &str, expires_in: Option<u64>) -> GrokTokens {
    GrokTokens {
        access_token: access_token.into(),
        refresh_token: Some("refresh".into()),
        expires_at: expires_in.map(|secs| SystemTime::now() + Duration::from_secs(secs)),
        connected_at: None,
    }
}

// ── serde round-trip ────────────────────────────────────────────

#[test]
fn serde_round_trip_empty() {
    let keys = ApiKeys::default();
    let json = serde_json::to_string(&keys).unwrap();
    let deser: ApiKeys = serde_json::from_str(&json).unwrap();
    assert_eq!(keys, deser);
}

#[test]
fn serde_round_trip_with_provider_keys() {
    let keys = ApiKeys {
        openai: Some("sk-openai".into()),
        anthropic: Some("sk-ant-abc".into()),
        google: Some("AIzaSy123".into()),
        open_router: Some("sk-or-xxx".into()),
        custom_endpoints: Vec::new(),
    };
    let json = serde_json::to_string(&keys).unwrap();
    let deser: ApiKeys = serde_json::from_str(&json).unwrap();
    assert_eq!(keys, deser);
}

#[test]
fn serde_ignores_unknown_fields() {
    let json = r#"{"openai":"sk-x","unknown_field":"value","custom_endpoints":[]}"#;
    let keys: ApiKeys = serde_json::from_str(json).unwrap();
    assert_eq!(keys.openai, Some("sk-x".into()));
}

// ── has_any_key ─────────────────────────────────────────────────

#[test]
fn has_any_key_false_when_empty() {
    assert!(!ApiKeys::default().has_any_key());
}

#[test]
fn has_any_key_true_for_openai_only() {
    let keys = ApiKeys {
        openai: Some("sk-x".into()),
        ..Default::default()
    };
    assert!(keys.has_any_key());
}

#[test]
fn custom_endpoints_round_trip_through_secure_storage_payload() {
    let keys = ApiKeys {
        custom_endpoints: vec![CustomEndpoint {
            name: "Local gateway".into(),
            url: "http://127.0.0.1:8080/v1".into(),
            api_key: "local-key".into(),
            models: vec![CustomEndpointModel {
                name: "local-model".into(),
                alias: Some("Local".into()),
                config_key: "stable-model-id".into(),
            }],
        }],
        ..Default::default()
    };

    let json = serde_json::to_string(&keys).unwrap();
    let restored: ApiKeys = serde_json::from_str(&json).unwrap();

    assert_eq!(restored, keys);
    assert!(restored.has_any_key());
    assert!(restored.has_custom_endpoints());
    assert_eq!(
        restored.custom_endpoints[0].models[0].display_label(),
        "Local"
    );
}

#[test]
fn custom_endpoint_models_preserve_ids_and_generate_missing_ids() {
    let models = ApiKeyManager::custom_endpoint_models(vec![
        ("preserved".into(), None, Some("existing-model-id".into())),
        ("generated".into(), Some("Alias".into()), None),
    ]);

    assert_eq!(models[0].config_key, "existing-model-id");
    assert!(!models[1].config_key.is_empty());
    assert_eq!(models[1].display_label(), "Alias");
}

#[test]
fn provider_key_count_counts_non_blank_provider_keys_only() {
    let keys = ApiKeys {
        openai: Some("sk-o".into()),
        anthropic: Some("   ".into()),
        google: Some("AIza".into()),
        open_router: None,
        custom_endpoints: vec![CustomEndpoint {
            name: "Local".into(),
            url: "http://127.0.0.1:8080/v1".into(),
            api_key: "local-key".into(),
            models: Vec::new(),
        }],
    };

    assert_eq!(keys.provider_key_count(), 2);
}

// ── grok expiry + blocking-refresh eligibility ──────────────────

#[cfg(not(target_family = "wasm"))]
fn expired_grok_tokens() -> GrokTokens {
    // Already past hard expiry, with a refresh token available.
    GrokTokens {
        access_token: "stale-access".into(),
        refresh_token: Some("refresh".into()),
        expires_at: Some(SystemTime::now() - Duration::from_secs(60)),
        connected_at: None,
    }
}

#[test]
fn grok_is_expired_semantics() {
    // Past hard expiry.
    assert!(
        GrokTokens {
            expires_at: Some(SystemTime::now() - Duration::from_secs(1)),
            ..Default::default()
        }
        .is_expired()
    );
    // Still valid, even if near expiry (within the proactive lead window).
    assert!(!grok_tokens("tok", Some(60)).is_expired());
    // Unknown expiry is never considered expired.
    assert!(!grok_tokens("tok", None).is_expired());
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn grok_expired_refresh_token_returns_token_when_expired() {
    let mgr = make_manager_with_grok(ApiKeys::default(), Some(expired_grok_tokens()));
    assert_eq!(
        mgr.grok_expired_refresh_token(true),
        Some("refresh".to_string())
    );
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn grok_expired_refresh_token_none_when_byo_disabled() {
    let mgr = make_manager_with_grok(ApiKeys::default(), Some(expired_grok_tokens()));
    assert_eq!(mgr.grok_expired_refresh_token(false), None);
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn grok_expired_refresh_token_none_when_near_expiry_but_valid() {
    // Within the proactive lead window but not yet expired: the background timer
    // handles this, so the blocking path stays out of it.
    let mgr = make_manager_with_grok(ApiKeys::default(), Some(grok_tokens("near", Some(60))));
    assert_eq!(mgr.grok_expired_refresh_token(true), None);
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn grok_expired_refresh_token_none_when_no_tokens() {
    let mgr = make_manager_with_grok(ApiKeys::default(), None);
    assert_eq!(mgr.grok_expired_refresh_token(true), None);
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn grok_expired_refresh_token_none_when_no_refresh_token() {
    let mut tokens = expired_grok_tokens();
    tokens.refresh_token = None;
    let mgr = make_manager_with_grok(ApiKeys::default(), Some(tokens));
    assert_eq!(mgr.grok_expired_refresh_token(true), None);
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn grok_expired_refresh_token_none_when_no_expiry() {
    // A token with no known expiry is never considered expired.
    let mgr = make_manager_with_grok(ApiKeys::default(), Some(grok_tokens("no-expiry", None)));
    assert_eq!(mgr.grok_expired_refresh_token(true), None);
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn grok_expired_refresh_token_ignores_in_flight_refresh() {
    // Eligibility is independent of whether a refresh is already running: a
    // request must still be able to attach to the in-flight refresh (that
    // coordination happens in `begin_expired_grok_refresh`), rather than being
    // told no refresh is needed and sending the expired token.
    let mut mgr = make_manager_with_grok(ApiKeys::default(), Some(expired_grok_tokens()));
    mgr.grok_refresh_waiters = Some(Vec::new());
    assert_eq!(
        mgr.grok_expired_refresh_token(true),
        Some("refresh".to_string())
    );
}
