//! Notification center (mailbox + toast).
//!
//! Rebuilt after being mistakenly deleted in 002ce467 cloud-removal; keeps only the local,
//! cloud-independent paths:
//! - completion/error notifications from the app's own BYOP agent (Oz)
//! - status notifications from third-party CLI agents (Claude Code / Codex / DeepSeek, etc.)
//!
//! Module layout:
//! - `item`         data model (`NotificationItem` / `NotificationItems`, etc.)
//! - `item_rendering` single-notification UI (shared by mailbox and toast)
//! - `model`        the singleton `NotificationsModel` (subscribes to the history / cli session
//!   models and produces notifications)
//! - `view`         `NotificationMailboxView` (the mailbox main panel)
//! - `toast_stack`  `AgentNotificationToastStack` (bottom-right toast)
//! - `telemetry`    notification-center telemetry events (`NotificationsTelemetryEvent`)

pub(crate) mod item;
pub(crate) mod item_rendering;
pub mod model;
pub(crate) mod telemetry;
pub mod toast_stack;
pub mod view;

pub(crate) use item::{
    NotificationCategory, NotificationFilter, NotificationId, NotificationItem, NotificationItems,
    NotificationSourceAgent,
};
pub use toast_stack::AgentNotificationToastStack;
pub use view::{NotificationMailboxView, NotificationMailboxViewEvent};

pub fn init(app: &mut warpui::AppContext) {
    NotificationMailboxView::init(app);
}
