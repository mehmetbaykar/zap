use warpui::App;

use super::*;
use crate::ai::agent_providers::AgentProviderSecrets;
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::cloud_object::model::persistence::ObjectStoreModel;
use crate::cloud_object::update_manager::UpdateManager;
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
        custom_model_routers: Vec::new(),
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

// -- Disable-aware default fallback tests --

fn server_llm(id: &str, disable_reason: Option<DisableReason>) -> LLMInfo {
    LLMInfo {
        display_name: id.to_string(),
        base_model_name: id.to_string(),
        id: id.into(),
        reasoning_level: None,
        usage_metadata: LLMUsageMetadata {
            request_multiplier: 1,
            credit_multiplier: None,
        },
        description: None,
        disable_reason,
        vision_supported: false,
        spec: None,
        provider: LLMProvider::Unknown,
        host_configs: HashMap::new(),
        discount_percentage: None,
        context_window: LLMContextWindow::default(),
    }
}

fn available(default_id: &str, choices: Vec<LLMInfo>) -> AvailableLLMs {
    AvailableLLMs {
        default_id: default_id.into(),
        choices,
        preferred_codex_model_id: None,
    }
}

#[test]
fn active_models_fall_back_to_usable_choice_or_custom_endpoint_when_default_disabled() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);
        app.add_singleton_model(UserWorkspaces::default_mock);
        app.add_singleton_model(UpdateManager::mock);
        app.add_singleton_model(ObjectStoreModel::mock);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());

        app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        let llm_preferences = app.add_singleton_model(LLMPreferences::new);

        let custom_model_id = LLMId::from("custom-config-key");
        ApiKeyManager::handle(&app).update(&mut app, |api_key_manager, ctx| {
            api_key_manager.add_custom_endpoint(
                "local".to_string(),
                "https://example.com/v1".to_string(),
                "test-key".to_string(),
                vec![(
                    "custom-model".to_string(),
                    None,
                    Some(custom_model_id.to_string()),
                )],
                ctx,
            );
        });

        // The base/coding default is admin-disabled but another hosted choice
        // is usable; every hosted CLI agent choice is admin-disabled.
        let models = ModelsByFeature {
            agent_mode: available(
                "auto",
                vec![
                    server_llm("auto", Some(DisableReason::AdminDisabled)),
                    server_llm("gpt-x", None),
                ],
            ),
            coding: available(
                "auto",
                vec![
                    server_llm("auto", Some(DisableReason::AdminDisabled)),
                    server_llm("gpt-x", None),
                ],
            ),
            cli_agent: Some(available(
                "cli-agent-auto",
                vec![server_llm(
                    "cli-agent-auto",
                    Some(DisableReason::AdminDisabled),
                )],
            )),
            computer_use: None,
        };
        llm_preferences.update(&mut app, |preferences, ctx| {
            preferences.update_feature_model_choices(Ok(models), ctx);
        });

        llm_preferences.read(&app, |preferences, app| {
            // Falls back to the first usable hosted choice.
            assert_eq!(
                preferences.get_active_base_model(app, None).id.as_str(),
                "gpt-x"
            );
            assert_eq!(
                preferences.get_active_coding_model(app, None).id.as_str(),
                "gpt-x"
            );
            // No usable hosted CLI choice → falls back to the custom endpoint.
            assert_eq!(
                preferences.get_active_cli_agent_model(app, None).id,
                custom_model_id
            );
        });
    });
}

#[test]
fn active_models_use_default_when_usable() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);
        app.add_singleton_model(UserWorkspaces::default_mock);
        app.add_singleton_model(UpdateManager::mock);
        app.add_singleton_model(ObjectStoreModel::mock);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());

        app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        let llm_preferences = app.add_singleton_model(LLMPreferences::new);

        let models = ModelsByFeature {
            agent_mode: available(
                "auto",
                vec![server_llm("auto", None), server_llm("gpt-x", None)],
            ),
            coding: available("auto", vec![server_llm("auto", None)]),
            cli_agent: Some(available(
                "cli-agent-auto",
                vec![server_llm("cli-agent-auto", None)],
            )),
            computer_use: None,
        };
        llm_preferences.update(&mut app, |preferences, ctx| {
            preferences.update_feature_model_choices(Ok(models), ctx);
        });

        llm_preferences.read(&app, |preferences, app| {
            assert_eq!(
                preferences.get_active_base_model(app, None).id.as_str(),
                "auto"
            );
            assert_eq!(
                preferences
                    .get_active_cli_agent_model(app, None)
                    .id
                    .as_str(),
                "cli-agent-auto"
            );
        });
    });
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

#[test]
fn reconcile_preserves_custom_endpoint_models_not_configured_locally() {
    // Regression test for QUALITY-866: a profile whose model was set to a custom
    // endpoint on device A should NOT be reset when device B syncs that profile
    // but does not have the corresponding custom endpoint configured.
    //
    // Before the fix, `reconcile_disabled_model_preferences` would clear any model
    // ID that couldn't be resolved locally, causing the profile to revert to Auto
    // and syncing that change back to cloud — erasing the user's setting on device A.
    //
    // The `context_window_limit` clear is a separately-guarded branch in
    // `reconcile_disabled_model_preferences` (gated on
    // `preferred_base_model_is_recognized`), so this test also sets a limit and
    // asserts it is preserved for the unrecognized custom endpoint ID.
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(AgentProviderSecrets::new);
        app.add_singleton_model(UserWorkspaces::default_mock);
        app.add_singleton_model(UpdateManager::mock);
        app.add_singleton_model(ObjectStoreModel::mock);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());

        let profiles_model = app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        let llm_preferences = app.add_singleton_model(LLMPreferences::new);

        // Simulate a model ID from a custom endpoint on another device.
        // This device (device B) does NOT have the endpoint configured locally.
        let remote_custom_model_id = LLMId::from("a1b2c3d4-5e6f-7890-abcd-ef1234567890");
        // Intentionally skip adding the endpoint to ApiKeyManager.

        let default_profile_id =
            profiles_model.read(&app, |profiles, _| profiles.default_profile_id());
        // Also set a context window limit so the separately-guarded
        // `context_window_limit` clear branch in `reconcile_disabled_model_preferences`
        // is exercised: it must NOT clear the limit for an unrecognized model ID.
        let preserved_context_window_limit: u32 = 200_000;
        profiles_model.update(&mut app, |profiles, ctx| {
            profiles.set_base_model(
                default_profile_id,
                Some(remote_custom_model_id.clone()),
                ctx,
            );
            profiles.set_coding_model(
                default_profile_id,
                Some(remote_custom_model_id.clone()),
                ctx,
            );
            profiles.set_cli_agent_model(
                default_profile_id,
                Some(remote_custom_model_id.clone()),
                ctx,
            );
            profiles.set_context_window_limit(
                default_profile_id,
                Some(preserved_context_window_limit),
                ctx,
            );
        });

        // Trigger a model list refresh (as happens on login, network reconnect, etc.).
        llm_preferences.update(&mut app, |preferences, ctx| {
            preferences.update_feature_model_choices(Ok(ModelsByFeature::default()), ctx);
        });

        // The model IDs should be PRESERVED even though no matching custom endpoint
        // is configured on this device.
        profiles_model.read(&app, |profiles, ctx| {
            let profile = profiles.default_profile(ctx);
            assert_eq!(
                profile.data().base_model.as_ref(),
                Some(&remote_custom_model_id),
                "base_model must be preserved for unknown custom endpoint IDs (cross-device sync)"
            );
            assert_eq!(
                profile.data().coding_model.as_ref(),
                Some(&remote_custom_model_id),
                "coding_model must be preserved for unknown custom endpoint IDs (cross-device sync)"
            );
            assert_eq!(
                profile.data().cli_agent_model.as_ref(),
                Some(&remote_custom_model_id),
                "cli_agent_model must be preserved for unknown custom endpoint IDs (cross-device sync)"
            );
            assert_eq!(
                profile.data().context_window_limit,
                Some(preserved_context_window_limit),
                "context_window_limit must be preserved for unknown custom endpoint IDs (cross-device sync)"
            );
        });
    });
}

// -- tui_agent_model_info tests --

fn agent_llm(id: &str, display_name: &str) -> LLMInfo {
    LLMInfo {
        display_name: display_name.to_owned(),
        base_model_name: display_name.to_owned(),
        id: id.into(),
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
    }
}

/// Preferences whose agent-mode models are a server-style list with an
/// `"auto"` default plus one concrete model.
fn preferences_for_tui_tests() -> LLMPreferences {
    let agent_mode = AvailableLLMs::new(
        "auto".into(),
        vec![
            agent_llm("auto", "auto (cost-efficient)"),
            agent_llm("claude-opus", "Opus"),
        ],
        None,
    )
    .expect("choices are non-empty");
    LLMPreferences {
        models_by_feature: ModelsByFeature {
            agent_mode,
            ..Default::default()
        },
        last_update: None,
        base_llm_for_terminal_view: HashMap::new(),
        reasoning_effort_per_terminal: HashMap::new(),
        last_used_reasoning: HashMap::new(),
        custom_llms: Vec::new(),
        custom_model_routers: Vec::new(),
    }
}

fn tui_agent_model_test(f: impl FnOnce(&LLMPreferences)) {
    f(&preferences_for_tui_tests());
}

#[test]
fn tui_agent_model_auto_resolves_to_the_default_model() {
    tui_agent_model_test(|preferences| {
        assert_eq!(preferences.tui_agent_model_info("auto").id.as_str(), "auto");
    });
}

#[test]
fn tui_agent_model_known_id_resolves_to_that_model() {
    tui_agent_model_test(|preferences| {
        let info = preferences.tui_agent_model_info("claude-opus");
        assert_eq!(info.id.as_str(), "claude-opus");
        assert_eq!(info.display_name, "Opus");
    });
}

#[test]
fn tui_agent_model_unknown_id_falls_back_to_the_default_model() {
    tui_agent_model_test(|preferences| {
        assert_eq!(
            preferences.tui_agent_model_info("not-a-model").id.as_str(),
            "auto"
        );
    });
}
