use crate::channel::ChannelState;

// Upstream Warp's docs site/Slack/privacy policy no longer apply to the Zap fork. The fork's own
// GitHub repository is its documentation home, so docs/help links point there. The channels that
// have no fork equivalent stay empty; the platform `open_url` treats an empty URL as a silent no-op
// (see `warpui` platform/mac/window.rs and windowing/winit/delegate.rs) so such links never error.
pub const USER_DOCS_URL: &str = "https://github.com/mehmetbaykar/zap";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const GITHUB_ISSUES_URL: &str = "https://github.com/mehmetbaykar/zap/issues";
pub const SLACK_URL: &str = "";
pub const PRIVACY_POLICY_URL: &str = "";

pub fn feedback_form_url() -> String {
    let mut url = url::Url::parse("https://github.com/mehmetbaykar/zap/issues/new/choose")
        .expect("Should not fail to parse");
    if let Some(version) = ChannelState::app_version() {
        url.query_pairs_mut().append_pair("zap-version", version);
    }
    url.query_pairs_mut()
        .append_pair("os-version", &os_info::get().version().to_string());
    url.to_string()
}
