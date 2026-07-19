//! Module containing the definition of [`ActiveFileModel`],
//! which tracks the currently focused file across an entire PaneGroup.

use warpui::{Entity, ModelContext};

use super::buffer_location::BufferLocation;

/// Events emitted by the ActiveFileModel.
#[derive(Debug, Clone)]
pub enum ActiveFileEvent {
    /// A new file became focused.
    ActiveFileChanged { location: BufferLocation },
    /// Zap: a file failed to load (it no longer exists at its recorded
    /// location). The file tree uses this to refresh stale remote entries.
    LoadFailed { location: BufferLocation },
}

/// Model that tracks the currently focused file.
#[derive(Default)]
pub struct ActiveFileModel {
    /// The currently focused file, if any.
    active_file: Option<BufferLocation>,
}

impl Entity for ActiveFileModel {
    type Event = ActiveFileEvent;
}

impl ActiveFileModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the currently active file, if any.
    pub fn active_file(&self) -> Option<&BufferLocation> {
        self.active_file.as_ref()
    }

    /// Set the currently active file.
    pub fn active_file_changed(&mut self, location: BufferLocation, ctx: &mut ModelContext<Self>) {
        // Only emit event if the active file changed.
        if self.active_file.as_ref() != Some(&location) {
            self.active_file = Some(location.clone());
            ctx.emit(ActiveFileEvent::ActiveFileChanged { location });
            ctx.notify();
        }
    }

    /// Zap: report that a file failed to load at its recorded location.
    pub fn active_file_load_failed(
        &mut self,
        location: BufferLocation,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.active_file.as_ref() == Some(&location) {
            self.active_file = None;
        }
        ctx.emit(ActiveFileEvent::LoadFailed { location });
        ctx.notify();
    }
}
