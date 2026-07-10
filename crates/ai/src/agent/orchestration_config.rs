/// Client-side representation of the orchestration config attached to a
/// conversation via `OrchestrationConfigSnapshot`.
///
/// Mirrors the proto `OrchestrationConfig` but uses Rust-native types
/// to keep view / model code free of proto imports.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OrchestrationConfig {
    pub model_id: String,
    pub harness_type: String,
    pub execution_mode: OrchestrationExecutionMode,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum OrchestrationExecutionMode {
    Local,
    Remote {
        environment_id: String,
        worker_host: String,
    },
}

impl OrchestrationExecutionMode {
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }
}

/// User's approval state for orchestration on the active config.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum OrchestrationConfigStatus {
    /// No `OrchestrationConfigSnapshot` has been seen yet.
    #[default]
    None,
    Approved,
    Disapproved,
}

impl OrchestrationConfigStatus {
    pub fn is_approved(&self) -> bool {
        matches!(self, Self::Approved)
    }

    pub fn is_disapproved(&self) -> bool {
        matches!(self, Self::Disapproved)
    }
}

