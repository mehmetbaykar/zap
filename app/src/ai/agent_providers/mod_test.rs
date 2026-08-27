//! Smoke tests for BYOP provider configuration and lookup.

use ai::LLMId;
use settings::Setting;
use warpui::{App, SingletonEntity};

use crate::ai::agent_providers::{AgentProviderSecrets, llm_id, lookup_byop};
use crate::ai::llms::{DisableReason, LLMPreferences};
use crate::auth::{AuthManager, AuthStateProvider};
use crate::network::NetworkStatus;
use crate::settings::{AISettings, AgentProvider, AgentProviderApiType, AgentProviderModel};
use crate::test_util::settings::initialize_settings_for_tests;
use crate::workspaces::user_workspaces::UserWorkspaces;

fn sample_provider(id: &str) -> AgentProvider {
    AgentProvider {
        id: id.to_owned(),
        name: "Test Ollama".to_owned(),
        kind: Default::default(),
        api_type: AgentProviderApiType::Ollama,
        base_url: "http://localhost:11434".to_owned(),
        models: vec![AgentProviderModel::from_id("llama3.2".to_owned())],
        extra_headers: Vec::new(),
    }
}

fn init_byop_test_app(app: &mut warpui::App) {
    initialize_settings_for_tests(app);
    // This fork's LLMPreferences reconciles disabled-model preferences on model-list
    // updates (upstream warp #10085), which reads the execution-profiles singleton;
    // register it like the other model-refresh tests do.
    app.add_singleton_model(crate::cloud_object::model::persistence::ObjectStoreModel::mock);
    app.add_singleton_model(|_| {
        crate::ai::mcp::templatable_manager::TemplatableMCPServerManager::default()
    });
    app.add_singleton_model(|ctx| {
        crate::ai::execution_profiles::profiles::AIExecutionProfilesModel::new(
            &crate::LaunchMode::new_for_unit_test(),
            ctx,
        )
    });
    app.add_singleton_model(AgentProviderSecrets::new);
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AuthManager::new_for_test);
    app.add_singleton_model(UserWorkspaces::default_mock);
    app.add_singleton_model(LLMPreferences::new);
}

#[test]
fn smoke_build_byop_models_by_feature_exposes_configured_models() {
    App::test((), |mut app| async move {
        init_byop_test_app(&mut app);

        let provider_id = "provider-smoke-1";
        app.update(|ctx| {
            AISettings::handle(ctx).update(ctx, |settings, ctx| {
                let _ = settings
                    .agent_providers
                    .set_value(vec![sample_provider(provider_id)], ctx);
            });
        });

        app.read(|ctx| {
            let choices: Vec<_> = LLMPreferences::as_ref(ctx)
                .get_base_llm_choices_for_agent_mode()
                .collect();
            assert_eq!(choices.len(), 1, "expected one BYOP model in picker");
            assert!(
                choices[0].disable_reason.is_none(),
                "valid provider should not be disabled"
            );
            assert_eq!(
                choices[0].id.as_str(),
                llm_id::encode(provider_id, "llama3.2").as_str()
            );
        });
    });
}

#[test]
fn smoke_build_byop_models_by_feature_uses_placeholder_when_misconfigured() {
    App::test((), |mut app| async move {
        init_byop_test_app(&mut app);

        app.read(|ctx| {
            let default = LLMPreferences::as_ref(ctx).get_default_base_model();
            assert_eq!(
                default.disable_reason,
                Some(DisableReason::Unavailable),
                "empty config should surface placeholder entry"
            );
        });
    });
}

#[test]
fn smoke_build_byop_models_by_feature_skips_empty_base_url() {
    App::test((), |mut app| async move {
        init_byop_test_app(&mut app);

        app.update(|ctx| {
            AISettings::handle(ctx).update(ctx, |settings, ctx| {
                let mut broken = sample_provider("broken");
                broken.base_url.clear();
                let _ = settings.agent_providers.set_value(vec![broken], ctx);
            });
        });

        app.read(|ctx| {
            let default = LLMPreferences::as_ref(ctx).get_default_base_model();
            assert_eq!(
                default.disable_reason,
                Some(DisableReason::Unavailable),
                "provider with empty base_url must not appear as selectable model"
            );
        });
    });
}

#[test]
fn smoke_lookup_byop_resolves_provider_and_model_without_api_key() {
    App::test((), |mut app| async move {
        init_byop_test_app(&mut app);

        let provider_id = "provider-lookup-1";
        app.update(|ctx| {
            AISettings::handle(ctx).update(ctx, |settings, ctx| {
                let _ = settings
                    .agent_providers
                    .set_value(vec![sample_provider(provider_id)], ctx);
            });
        });

        let encoded = llm_id::encode(provider_id, "llama3.2");
        app.read(|ctx| {
            let (provider, api_key, model_id) =
                lookup_byop(ctx, &encoded).expect("lookup_byop should resolve configured model");
            assert_eq!(provider.id, provider_id);
            assert_eq!(model_id, "llama3.2");
            assert!(api_key.is_empty(), "Ollama path allows empty API key");
        });
    });
}

#[test]
fn smoke_lookup_byop_resolves_custom_endpoint_config_key_locally() {
    App::test((), |mut app| async move {
        init_byop_test_app(&mut app);

        let config_key = "custom-model-config-key";
        app.update(|ctx| {
            ai::api_keys::ApiKeyManager::handle(ctx).update(ctx, |manager, ctx| {
                manager.add_custom_endpoint(
                    ai::api_keys::CustomEndpointParams {
                        name: "Local gateway".to_owned(),
                        url: "http://127.0.0.1:8080/v1".to_owned(),
                        api_key: "local-secret".to_owned(),
                        models: vec![(
                            "upstream-model".to_owned(),
                            Some("Friendly model".to_owned()),
                            Some(config_key.to_owned()),
                        )],
                        schema: Default::default(),
                    },
                    ctx,
                );
            });
        });

        app.read(|ctx| {
            let (provider, api_key, model_id) =
                lookup_byop(ctx, &LLMId::from(config_key)).expect("custom endpoint should route");

            assert_eq!(provider.id, format!("custom-endpoint:{config_key}"));
            assert_eq!(provider.name, "Local gateway");
            assert_eq!(provider.api_type, AgentProviderApiType::OpenAi);
            assert_eq!(provider.base_url, "http://127.0.0.1:8080/v1");
            assert_eq!(provider.models.len(), 1);
            assert_eq!(provider.models[0].name, "Friendly model");
            assert_eq!(provider.models[0].id, "upstream-model");
            assert_eq!(api_key, "local-secret");
            assert_eq!(model_id, "upstream-model");
        });
    });
}

#[test]
fn smoke_lookup_byop_returns_none_for_unknown_id() {
    App::test((), |mut app| async move {
        init_byop_test_app(&mut app);

        app.read(|ctx| {
            assert!(lookup_byop(ctx, &LLMId::from("byop:missing:model")).is_none());
            assert!(lookup_byop(ctx, &LLMId::from("not-byop")).is_none());
        });
    });
}
