use std::path::{Path, PathBuf};

use ai::skills::SkillPathOrigin;
use warp_util::local_or_remote_path::LocalOrRemotePath;

mod telemetry;
pub use telemetry::{SkillOpenOrigin, SkillTelemetryEvent};
#[cfg(all(not(target_family = "wasm"), feature = "local_fs"))]
mod remote;
#[cfg(all(not(target_family = "wasm"), feature = "local_fs"))]
pub(crate) use remote::{bundled_skills_snapshot_protos, wire_remote_bundled_skills};
#[cfg(feature = "local_fs")]
mod bundled;
#[cfg(all(not(target_family = "wasm"), feature = "local_fs"))]
pub(crate) use bundled::BundledSkill;
cfg_if::cfg_if! {
    if #[cfg(not(feature = "local_fs"))] {
        mod dummy_skill_manager;
        pub use dummy_skill_manager::{
            SkillInventoryDuplicate, SkillInventoryItem, SkillManager, SkillManagerEvent,
        };
    }
}

pub use ai::skills::SkillReference;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActiveSkillLookupError {
    #[error("Bundled skills are not available on this remote session")]
    BundledSkillsUnavailable,
    #[error("Skill not found: {reference}")]
    NotFound { reference: SkillReference },
}

impl ActiveSkillLookupError {
    pub(crate) fn for_reference(reference: &SkillReference, path_origin: &SkillPathOrigin) -> Self {
        if matches!(path_origin, SkillPathOrigin::Unavailable)
            && matches!(reference, SkillReference::BundledSkillId(_))
        {
            Self::BundledSkillsUnavailable
        } else {
            Self::NotFound {
                reference: reference.clone(),
            }
        }
    }
}

#[cfg(not(target_family = "wasm"))]
mod global_skills;

mod listed_skill;
pub use listed_skill::SkillDescriptor;

mod skill_utils;
pub use skill_utils::{
    icon_override_for_skill_name, list_skills, render_skill_button, skill_path_from_location,
};
pub trait SkillPathQuery {
    fn to_skill_location(&self) -> LocalOrRemotePath;
}

impl SkillPathQuery for LocalOrRemotePath {
    fn to_skill_location(&self) -> LocalOrRemotePath {
        self.clone()
    }
}

impl SkillPathQuery for Path {
    fn to_skill_location(&self) -> LocalOrRemotePath {
        LocalOrRemotePath::Local(self.to_path_buf())
    }
}

impl SkillPathQuery for PathBuf {
    fn to_skill_location(&self) -> LocalOrRemotePath {
        LocalOrRemotePath::Local(self.clone())
    }
}

#[cfg(not(target_family = "wasm"))]
mod resolve_skill_spec;
#[cfg(not(target_family = "wasm"))]
pub use resolve_skill_spec::{
    clone_repo_for_skill, resolve_skill_spec, ResolveSkillError, ResolvedSkill,
};

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        mod skill_manager;
        pub use skill_manager::{
            extract_skill_parent_directory, SkillInventoryDuplicate, SkillInventoryItem,
            SkillManager, SkillManagerEvent,
        };
        #[allow(unused_imports)]
        pub use skill_manager::SkillWatcher;
    }
}
