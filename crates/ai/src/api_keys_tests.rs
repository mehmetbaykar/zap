use super::*;

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
