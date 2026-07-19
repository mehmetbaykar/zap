//! Telemetry events for the agent management view.
//!
//! Zap: telemetry sending is physically removed (`send_telemetry_from_ctx!` is a no-op shim,
//! see `crates/warp_core/src/telemetry.rs`), and the `TelemetryEvent`/`register_telemetry_event!`
//! machinery upstream used to describe/ship these events no longer exists in this fork (see
//! `app/src/notifications/telemetry.rs` for the established precedent: a bare enum, no trait
//! impl). This mirrors that pattern rather than porting the full upstream trait ceremony.
//!
//! Variants that only make sense for Warp-hosted cloud runs (spawning a cloud agent; the
//! conversation "tombstone" continue-in-cloud/continue-locally flows, which are a separate
//! feature not ported here) are dropped — nothing in this fork's port would ever construct them,
//! which would otherwise be a guaranteed dead-code warning. `CloudRunOpened`/`CloudRunCancelled`
//! are kept (renamed in spirit only via doc comment, not in wire format) because they still have
//! real call sites here: task-backed (`AmbientRun`) entries in the dashboard, even though in this
//! fork such entries are always local ambient-agent tasks, never literally cloud-hosted ones.

use serde::Serialize;

/// Which setup-guide/empty-state workflow step the user interacted with.
///
/// Zap has no cloud onboarding steps (create environment, Slack/Linear integration); this is
/// kept only for shape-compatibility in case the empty state grows local guidance steps later.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupGuideStep {
    /// Quick start banner: Visit Oz
    VisitOz,
}

/// Where the item was opened from
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenedFrom {
    ManagementView,
    ConversationList,
    DetailsPanel,
}

/// Type of artifact clicked
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Plan,
    Branch,
    PullRequest,
    File,
}

/// Type of filter changed
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterType {
    Owner,
    Status,
    Source,
    CreatedOn,
    Creator,
    Harness,
}

/// Telemetry events for the agent management view.
#[derive(Serialize, Debug)]
pub enum AgentManagementTelemetryEvent {
    /// User toggled the agent management view open or closed
    ViewToggled { is_open: bool },
    /// User opened the (local) empty-state / getting-started guide
    OpenSetupGuide,
    /// User dismissed the empty-state / getting-started guide
    DismissSetupGuide,
    /// User spawned a new local agent
    SpawnNewLocalAgent,
    /// User opened the agent type selector modal
    AgentTypeSelectorOpened,
    /// User opened a conversation
    ConversationOpened {
        conversation_id: String,
        opened_from: OpenedFrom,
    },
    /// User opened an ambient-agent run
    CloudRunOpened {
        task_id: String,
        opened_from: OpenedFrom,
    },
    /// User clicked an artifact button
    ArtifactClicked { artifact_type: ArtifactType },
    /// User changed a filter
    FilterChanged { filter_type: FilterType },
    /// User clicked an item details button
    DetailsViewed {
        item_id: String,
        viewed_from: OpenedFrom,
    },
    /// User copied a conversation link
    ConversationLinkCopied {
        conversation_id: String,
        copied_from: OpenedFrom,
    },
    /// User copied a session link
    SessionLinkCopied {
        task_id: String,
        copied_from: OpenedFrom,
    },
    /// User cancelled an ambient-agent run
    CloudRunCancelled { task_id: String },
    /// User forked a conversation
    ConversationForked { conversation_id: String },
}
