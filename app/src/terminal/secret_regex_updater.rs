use warpui::{Entity, ModelContext, SingletonEntity};

use crate::settings::{CustomSecretRegex, PrivacySettings, PrivacySettingsChangedEvent};
use crate::terminal::model::set_user_and_enterprise_secret_regexes;

/// Dummy singleton model that is used to update the current set of custom regexes within the
/// terminal model. We do this via a singleton model since we only want to do this once any time
/// the custom secret regex list changes, which must be done independent of any view.
pub struct CustomSecretRegexUpdater;

impl CustomSecretRegexUpdater {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let updater = CustomSecretRegexUpdater;
        // Seed the recommended default secret patterns once. Upstream does this
        // from `handle_warp_drive_objects_loaded` after its cloud prefs finish
        // loading; the fork has no cloud-load phase, so the local equivalent is
        // here at startup, before the first sync into the secret matcher.
        // Without this, a fresh install has an empty pattern list and Safe Mode
        // detects nothing. `initialize_default_regexes_once` is guarded by the
        // persisted HasInitializedDefaultSecretRegexes flag, so users who
        // already customized (or cleared) their list are left untouched.
        PrivacySettings::handle(ctx).update(ctx, |settings, ctx| {
            settings.initialize_default_regexes_once(ctx);
        });
        // Initialize with current custom regexes (will be empty until safe mode is enabled)
        updater.update_custom_secret_regex_list(ctx);

        let privacy_settings = PrivacySettings::handle(ctx);
        ctx.subscribe_to_model(&privacy_settings, |me, _, evt, ctx| {
            if let PrivacySettingsChangedEvent::CustomSecretRegexList { .. } = evt {
                me.update_custom_secret_regex_list(ctx);
            }
        });
        updater
    }

    fn update_custom_secret_regex_list(&self, ctx: &mut ModelContext<Self>) {
        let privacy_settings = PrivacySettings::as_ref(ctx);

        // Get enterprise and user secrets separately
        let enterprise_secrets = privacy_settings
            .enterprise_secret_regex_list
            .iter()
            .map(CustomSecretRegex::pattern);

        let user_secrets = privacy_settings
            .user_secret_regex_list
            .iter()
            .map(CustomSecretRegex::pattern);

        set_user_and_enterprise_secret_regexes(user_secrets, enterprise_secrets);

        // Zap (Wave1-S4): the original telemetry-side `update_telemetry_secrets_regex` call
        // was removed along with `server/telemetry/secret_redaction.rs` as a whole. The visual blur of secret mode
        // is already fully covered via `set_user_and_enterprise_secret_regexes`; the telemetry-side
        // defence-in-depth redact is meaningless now that there is no longer any outbound path.
    }
}

impl Entity for CustomSecretRegexUpdater {
    type Event = ();
}

impl SingletonEntity for CustomSecretRegexUpdater {}
