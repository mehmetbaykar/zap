use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::{NonNilUuid, Uuid};

#[derive(Debug, thiserror::Error)]
#[error("Invalid task ID: {0}")]
pub struct ParseAmbientAgentTaskIdError(#[from] uuid::Error);

/// A globally unique ID for an ambient agent task.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AmbientAgentTaskId(NonNilUuid);

impl Display for AmbientAgentTaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for AmbientAgentTaskId {
    type Err = ParseAmbientAgentTaskIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = Uuid::try_parse(s)?;
        Ok(Self(NonNilUuid::try_from(uuid)?))
    }
}

impl AmbientAgentTaskId {
    /// Zap (localization, Phase 3b-4): generates a UUID v4 locally as the task_id, to avoid depending on a remote
    /// pre-create-task interface when the local harness starts a child task.
    ///
    /// Moved here alongside the type when upstream #15459 relocated `AmbientAgentTaskId`
    /// into `ai_types`; the tuple field is private to this crate, so the constructor has
    /// to live next to the type.
    pub fn new_local() -> Self {
        let uuid = Uuid::new_v4();
        // A UUID v4 is almost impossible to be nil (probability ~ 1/2^122), so expect indicates this is logically unreachable.
        let non_nil =
            NonNilUuid::try_from(uuid).expect("freshly generated UUID v4 must be non-nil");
        Self(non_nil)
    }
}
