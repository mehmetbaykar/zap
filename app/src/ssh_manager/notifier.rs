//! Global SSH tree-change broadcast — any view that changes the tree structure
//! (add / delete / rename / modify server fields) calls `notify` once, and
//! subscribers like SshManagerPanel refresh based on it.
//!
//! Same pattern as `KeybindingChangedNotifier`
//! (`app/src/settings_view/keybindings.rs:72`): empty struct + SingletonEntity +
//! a single Event variant.

use warpui::{Entity, SingletonEntity};

#[derive(Default)]
pub struct SshTreeChangedNotifier {}

impl SshTreeChangedNotifier {
    pub fn new() -> Self {
        Default::default()
    }
}

#[derive(Clone, Debug)]
pub enum SshTreeChangedEvent {
    /// The node list / server details have changed; needs to re-run list_nodes.
    TreeChanged,
}

impl Entity for SshTreeChangedNotifier {
    type Event = SshTreeChangedEvent;
}

impl SingletonEntity for SshTreeChangedNotifier {}
