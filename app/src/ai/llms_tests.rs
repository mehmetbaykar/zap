use warpui::App;

use super::*;
use crate::ai::agent_providers::AgentProviderSecrets;
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::auth::{AuthManager, AuthStateProvider};
use crate::cloud_object::model::persistence::ObjectStoreModel;
use crate::cloud_object::update_manager::UpdateManager;
use crate::network::NetworkStatus;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::LaunchMode;

// -- DisableReason::should_clear_preference tests --

#[test]
fn should_clear_preference_admin_disabled() {
    // AdminDisabled always clears, regardless of BYOK status.
    assert!(DisableReason::AdminDisabled.should_clear_preference(false));
    assert!(DisableReason::AdminDisabled.should_clear_preference(true));
}

#[test]
fn should_clear_preference_unavailable() {
    assert!(DisableReason::Unavailable.should_clear_preference(false));
    assert!(DisableReason::Unavailable.should_clear_preference(true));
}

#[test]
fn should_not_clear_preference_out_of_requests() {
    // Transient — never clears.
    assert!(!DisableReason::OutOfRequests.should_clear_preference(false));
    assert!(!DisableReason::OutOfRequests.should_clear_preference(true));
}

#[test]
fn should_not_clear_preference_provider_outage() {
    // Transient — never clears.
    assert!(!DisableReason::ProviderOutage.should_clear_preference(false));
    assert!(!DisableReason::ProviderOutage.should_clear_preference(true));
}

#[test]
fn should_clear_preference_requires_upgrade_without_byok() {
    // No BYOK key → server will reject → clear.
    assert!(DisableReason::RequiresUpgrade.should_clear_preference(false));
}

#[test]
fn should_not_clear_preference_requires_upgrade_with_byok() {
    // BYOK key present → server allows → keep.
    assert!(!DisableReason::RequiresUpgrade.should_clear_preference(true));
}

#[test]
fn llm_info_deserializes_without_base_model_name() {
    let raw = r#"{
            "display_name": "gpt-4o",
            "id": "gpt-4o",
            "usage_metadata": {
                "request_multiplier": 1,
                "credit_multiplier": null
            },
            "description": null,
            "disable_reason": null,
            "vision_supported": false,
            "spec": null,
            "provider": "Unknown"
        }"#;

    let info: LLMInfo = serde_json::from_str(raw).expect("should deserialize");
    assert_eq!(info.display_name, "gpt-4o");
    assert_eq!(info.base_model_name, "gpt-4o");
}

#[test]
fn llm_info_deserializes_host_configs_as_vec() {
    // Wire format from server: host_configs is a Vec
    let raw = r#"{
            "display_name": "gpt-4o",
            "id": "gpt-4o",
            "usage_metadata": { "request_multiplier": 1, "credit_multiplier": null },
            "provider": "OpenAI",
            "host_configs": [
                { "enabled": true, "model_routing_host": "DirectApi" },
                { "enabled": false, "model_routing_host": "AwsBedrock" }
            ]
        }"#;

    let info: LLMInfo = serde_json::from_str(raw).expect("should deserialize vec format");
    assert_eq!(info.display_name, "gpt-4o");
    assert_eq!(info.host_configs.len(), 2);
    assert!(
        info.host_configs
            .get(&LLMModelHost::DirectApi)
            .unwrap()
            .enabled
    );
    assert!(
        !info
            .host_configs
            .get(&LLMModelHost::AwsBedrock)
            .unwrap()
            .enabled
    );
}

#[test]
fn llm_info_round_trip_serializes_and_deserializes() {
    // Start with wire format (Vec)
    let wire_json = r#"{
            "display_name": "claude-3",
            "base_model_name": "claude-3",
            "id": "claude-3",
            "usage_metadata": { "request_multiplier": 2, "credit_multiplier": 1.5 },
            "description": "A powerful model",
            "vision_supported": true,
            "provider": "Anthropic",
            "host_configs": [
                { "enabled": true, "model_routing_host": "DirectApi" }
            ]
        }"#;

    // Deserialize from wire format
    let info: LLMInfo = serde_json::from_str(wire_json).expect("should deserialize");

    // Serialize (produces HashMap format)
    let serialized = serde_json::to_string(&info).expect("should serialize");

    // Deserialize again (from HashMap format)
    let round_tripped: LLMInfo =
        serde_json::from_str(&serialized).expect("should deserialize after round trip");

    assert_eq!(info, round_tripped);
}

#[test]
fn deserialized_available_llms_with_missing_default_does_not_panic() {
    // `AvailableLLMs::new()` guarantees `default_id` is one of `choices`, but
    // deserialization (e.g. a stale persisted cache or a server payload)
    // bypasses `new()`. Build such a struct, round-trip it through serde, and
    // confirm `default_llm_info()` falls back to the first choice instead of
    // panicking (Sentry: "Default LLM ID must be present in choices").
    let original = AvailableLLMs {
        default_id: "missing-default".into(),
        choices: vec![
            LLMInfo {
                display_name: "gpt-x".to_string(),
                base_model_name: "gpt-x".to_string(),
                id: "gpt-x".into(),
                reasoning_level: None,
                usage_metadata: LLMUsageMetadata {
                    request_multiplier: 1,
                    credit_multiplier: None,
                },
                description: None,
                disable_reason: None,
                vision_supported: false,
                spec: None,
                provider: LLMProvider::Unknown,
                host_configs: HashMap::new(),
                discount_percentage: None,
                context_window: LLMContextWindow::default(),
            },
            LLMInfo {
                display_name: "gpt-y".to_string(),
                base_model_name: "gpt-y".to_string(),
                id: "gpt-y".into(),
                reasoning_level: None,
                usage_metadata: LLMUsageMetadata {
                    request_multiplier: 1,
                    credit_multiplier: None,
                },
                description: None,
                disable_reason: None,
                vision_supported: false,
                spec: None,
                provider: LLMProvider::Unknown,
                host_configs: HashMap::new(),
                discount_percentage: None,
                context_window: LLMContextWindow::default(),
            },
        ],
        preferred_codex_model_id: None,
    };
    let json = serde_json::to_string(&original).expect("should serialize");
    let deserialized: AvailableLLMs = serde_json::from_str(&json).expect("should deserialize");

    assert_eq!(deserialized.default_id.as_str(), "missing-default");
    assert_eq!(deserialized.default_llm_info().id.as_str(), "gpt-x");
}

fn endpoint(
    name: &str,
    url: &str,
    api_key: &str,
    models: Vec<CustomEndpointModel>,
) -> CustomEndpoint {
    CustomEndpoint {
        name: name.into(),
        url: url.into(),
        api_key: api_key.into(),
        models,
    }
}

fn model(name: &str, alias: Option<&str>, config_key: &str) -> CustomEndpointModel {
    CustomEndpointModel {
        name: name.into(),
        alias: alias.map(str::to_owned),
        config_key: config_key.into(),
    }
}

#[test]
fn custom_llm_infos_built_from_endpoints() {
    let keys = ai::api_keys::ApiKeys {
        custom_endpoints: vec![endpoint(
            "My Endpoint",
            "https://x.io",
            "k",
            vec![
                model("gpt-4", Some("fast"), "uuid-1"),
                model("llama", None, "uuid-2"),
            ],
        )],
        ..Default::default()
    };

    let infos = build_custom_llm_infos(&keys);

    assert_eq!(infos.len(), 2);
    assert_eq!(infos[0].display_name, "fast");
    assert_eq!(infos[0].id.as_str(), "uuid-1");
    assert_eq!(
        infos[0].description.as_deref(),
        Some("Custom · My Endpoint")
    );
    assert!(infos[0]
        .host_configs
        .contains_key(&LLMModelHost::CustomEndpoint));
    assert_eq!(infos[1].display_name, "llama");
    assert_eq!(infos[1].id.as_str(), "uuid-2");
}

#[test]
fn custom_llm_display_name_uses_alias_or_model_name() {
    let keys = ai::api_keys::ApiKeys {
        custom_endpoints: vec![endpoint(
            "ep",
            "https://a.io",
            "k",
            vec![
                model("raw-alias", Some("My Alias"), "uuid-a"),
                model("raw-name", None, "uuid-b"),
            ],
        )],
        ..Default::default()
    };

    let infos = build_custom_llm_infos(&keys);

    assert_eq!(infos[0].display_name, "My Alias");
    assert_eq!(infos[1].display_name, "raw-name");
}

#[test]
fn custom_endpoint_usage_display_label_resolves_model_and_fallback() {
    let keys = ai::api_keys::ApiKeys {
        custom_endpoints: vec![endpoint(
            "ep",
            "https://a.io",
            "k",
            vec![model("raw-name", Some("Alias"), "uuid-alias")],
        )],
        ..Default::default()
    };
    let preferences = LLMPreferences {
        models_by_feature: ModelsByFeature::default(),
        last_update: None,
        base_llm_for_terminal_view: HashMap::new(),
        reasoning_effort_per_terminal: HashMap::new(),
        last_used_reasoning: HashMap::new(),
        custom_llms: build_custom_llm_infos(&keys),
    };

    assert_eq!(
        preferences.custom_endpoint_usage_display_label("uuid-alias"),
        "Alias"
    );
    assert_eq!(
        preferences.custom_endpoint_usage_display_label("unknown"),
        CUSTOM_ENDPOINT_USAGE_FALLBACK_LABEL
    );
    assert_eq!(preferences.custom_llm_choices().count(), 1);
}

#[test]
fn custom_llm_infos_skip_incomplete_endpoint_and_model_rows() {
    let keys = ai::api_keys::ApiKeys {
        custom_endpoints: vec![
            endpoint(
                "missing-key",
                "https://a.io",
                "",
                vec![model("m", None, "a")],
            ),
            endpoint(
                "ready",
                "https://b.io",
                "k",
                vec![model("missing-id", None, ""), model("m", None, "ready-id")],
            ),
        ],
        ..Default::default()
    };

    let infos = build_custom_llm_infos(&keys);

    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].id.as_str(), "ready-id");
}

#[test]
fn rebuilding_custom_llms_removes_deleted_models_and_endpoints() {
    let before = ai::api_keys::ApiKeys {
        custom_endpoints: vec![
            endpoint("keep", "https://a.io", "k", vec![model("a", None, "a")]),
            endpoint(
                "remove",
                "https://b.io",
                "k",
                vec![model("b", None, "b"), model("c", None, "c")],
            ),
        ],
        ..Default::default()
    };
    let after = ai::api_keys::ApiKeys {
        custom_endpoints: vec![endpoint(
            "keep",
            "https://a.io",
            "k",
            vec![model("a", None, "a")],
        )],
        ..Default::default()
    };

    assert_eq!(build_custom_llm_infos(&before).len(), 3);
    let infos = build_custom_llm_infos(&after);
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].id.as_str(), "a");
}

#[test]
fn reconcile_preserves_custom_models_saved_on_execution_profile() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(UpdateManager::mock);
        app.add_singleton_model(ObjectStoreModel::mock);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());
        app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        app.add_singleton_model(AgentProviderSecrets::new);
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(AuthManager::new_for_test);
        app.add_singleton_model(|_| NetworkStatus::new());
        app.add_singleton_model(UserWorkspaces::default_mock);

        let profiles_model = AIExecutionProfilesModel::handle(&app);
        let llm_preferences = app.add_singleton_model(LLMPreferences::new);
        let custom_model_id = LLMId::from("custom-model-config-key");

        ai::api_keys::ApiKeyManager::handle(&app).update(&mut app, |api_key_manager, ctx| {
            api_key_manager.add_custom_endpoint(
                "local".to_owned(),
                "https://example.com/v1".to_owned(),
                "test-key".to_owned(),
                vec![(
                    "custom-model".to_owned(),
                    Some("Custom Model".to_owned()),
                    Some(custom_model_id.to_string()),
                )],
                ctx,
            );
        });

        let default_profile_id =
            profiles_model.read(&app, |profiles, _| profiles.default_profile_id());
        profiles_model.update(&mut app, |profiles, ctx| {
            profiles.set_base_model(default_profile_id, Some(custom_model_id.clone()), ctx);
            profiles.set_coding_model(default_profile_id, Some(custom_model_id.clone()), ctx);
            profiles.set_cli_agent_model(default_profile_id, Some(custom_model_id.clone()), ctx);
        });

        llm_preferences.update(&mut app, |preferences, ctx| {
            preferences.update_feature_model_choices(Ok(ModelsByFeature::default()), ctx);
        });

        profiles_model.read(&app, |profiles, ctx| {
            let profile = profiles.default_profile(ctx);
            assert_eq!(profile.data().base_model.as_ref(), Some(&custom_model_id));
            assert_eq!(profile.data().coding_model.as_ref(), Some(&custom_model_id));
            assert_eq!(
                profile.data().cli_agent_model.as_ref(),
                Some(&custom_model_id)
            );
        });
    });
}
