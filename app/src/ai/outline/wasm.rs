use std::path::{Path, PathBuf};

use warpui::{Entity, ModelContext, SingletonEntity};

use super::OutlineStatus;

pub struct RepoOutlines {}

impl RepoOutlines {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {}
    }

    pub fn new_with_indexing_enabled(
        _indexing_enabled: bool,
        _ctx: &mut ModelContext<Self>,
    ) -> Self {
        Self {}
    }

    #[cfg(any(test, feature = "integration_tests"))]
    pub fn new_for_test(ctx: &mut ModelContext<Self>) -> Self {
        Self::new(ctx)
    }

    pub fn get_outline(&self, _path: &Path) -> Option<(&OutlineStatus, PathBuf)> {
        None
    }

    pub fn get_outline_for_repo(&self, _repo_path: &Path) -> Option<&OutlineStatus> {
        None
    }

    pub fn is_directory_indexed(&self, _directory: &Path) -> bool {
        false
    }
}

impl Entity for RepoOutlines {
    type Event = ();
}

impl SingletonEntity for RepoOutlines {}
