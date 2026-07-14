//! Stable-channel `warp-tui` binary.
//!
//! Mirrors `app/src/bin/stable.rs`: loads the `stable` channel config with no
//! additional feature flags, then hands off to the shared TUI entry point.

use anyhow::Result;
use warp_core::channel::{Channel, ChannelState};

fn main() -> Result<()> {
    let mut config = warp_channel_config::load_config!("stable");
    config.autoupdate_config = None;
    ChannelState::set(ChannelState::new(Channel::Stable, config));

    warp_tui::run()
}
