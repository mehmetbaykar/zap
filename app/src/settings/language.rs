//! User-interface language setting (persisted via settings.toml, applied to the
//! i18n loader at startup).
//!
//! The UI ships in English only. `System` follows the OS locale but falls back to
//! English, since English is the only bundled translation.

use enum_iterator::Sequence;
use serde::{Deserialize, Serialize};
use warp_core::settings::{macros::define_settings_group, SupportedPlatforms, SyncToCloud};

#[derive(
    Default,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Sequence,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "The language used in Zap's user interface.",
    rename_all = "snake_case"
)]
pub enum Language {
    /// Follow the system language; falls back to English (the only bundled UI language).
    #[default]
    #[schemars(description = "System default")]
    System,
    #[schemars(description = "English")]
    English,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Language::System => "System default",
            Language::English => "English",
        };
        write!(f, "{value}")
    }
}

impl Language {
    /// Convert to a BCP-47 locale string; `System` returns `None` (use system detection).
    pub fn to_locale_str(self) -> Option<&'static str> {
        match self {
            Language::System => None,
            Language::English => Some("en"),
        }
    }
}

define_settings_group!(LanguageSettings, settings: [
    language: LanguageState {
        type: Language,
        default: Language::System,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        private: false,
        storage_key: "Language",
        toml_path: "appearance.language",
        description: "The language used in Zap's user interface. Falls back to English when the chosen language is not fully translated.",
    },
]);
