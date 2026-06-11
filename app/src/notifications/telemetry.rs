//! Telemetry events for the in-app notification mailbox / toast stack.
//!
//! This is a minimally trimmed version of `AgentManagementTelemetryEvent`, which was deleted along
//! with everything else in 002ce467 cloud-removal; it keeps only the variant the notification
//! center (`item_rendering.rs`) actually still uses -- the artifact-click event + a tombstone that
//! no longer exists but whose schema is kept for backward compatibility / future rebuild.

use serde::Serialize;

/// Notification artifact type (for telemetry).
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Plan,
    Branch,
    PullRequest,
}

/// Notification-center telemetry events.
#[derive(Serialize, Debug)]
pub enum NotificationsTelemetryEvent {
    /// The user clicked an artifact button in a notification item (plan / branch / PR)
    ArtifactClicked { artifact_type: ArtifactType },
}
