//! Agent management dashboard: a full-view list of local interactive conversations and ambient
//! agent runs, with filtering and a details side panel.
//!
//! Zap adaptations from upstream:
//! - `agent_management_model` (upstream's `AgentNotificationsModel`, which populates the
//!   notification mailbox) is not ported: it is fully superseded by this fork's own
//!   `crate::notifications::model::NotificationsModel`, which already does the same job (see that
//!   module's doc comment for its own de-cloud history). Nothing here needs a second copy of it.
//! - `notifications` (per-feature mailbox/toast UI) is not ported for the same reason; see
//!   `crate::notifications`.
//! - `cloud_setup_guide_view` (cloud onboarding: create environment, Slack/Linear integration) is
//!   not ported; there is no cloud onboarding in this fork. Call sites that would have shown the
//!   setup guide render a plain local empty state instead (see `view.rs`).
pub(crate) mod agent_type_selector;
pub(crate) mod details_action_buttons;

pub(crate) mod telemetry;
pub(crate) mod view;

pub fn init(app: &mut warpui::AppContext) {
    view::init(app);
    agent_type_selector::init(app);
}
