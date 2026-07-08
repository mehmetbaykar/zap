use super::*;
use crate::settings_view::settings_widget_deeplink_target;

// -- warp://settings deeplink parsing ----------------------------------------

#[test]
fn test_settings_widget_deeplink_target() {
    assert_eq!(
        settings_widget_deeplink_target("global_hotkey").map(|(section, _)| section),
        Some(SettingsSection::Features),
    );
    // custom_router / CustomModelRouters are not ported (Zap has no model-router).
    assert!(settings_widget_deeplink_target("custom_router").is_none());
    #[cfg(not(target_family = "wasm"))]
    assert_eq!(
        settings_widget_deeplink_target("cli_agents").map(|(section, _)| section),
        Some(SettingsSection::ThirdPartyCLIAgents),
    );
    // Unknown / empty slugs are not linkable (allowlist only).
    assert!(settings_widget_deeplink_target("not_a_widget").is_none());
    assert!(settings_widget_deeplink_target("").is_none());
}

#[test]
fn test_settings_section_for_simple_subpage() {
    assert_eq!(
        settings_section_for_simple_subpage("appearance"),
        Some(SettingsSection::Appearance),
    );
    assert_eq!(
        settings_section_for_simple_subpage("warp_agent"),
        Some(SettingsSection::WarpAgent),
    );
    // Zap stubs: billing / platform / teams are not simple settings sub-pages.
    assert!(settings_section_for_simple_subpage("billing_and_usage").is_none());
    assert!(settings_section_for_simple_subpage("platform").is_none());
    assert!(settings_section_for_simple_subpage("not_a_subpage").is_none());
}
