use std::collections::HashSet;
use std::hash::Hash;

use warp_core::features::FeatureFlag;

#[derive(Clone, Copy, Debug)]
pub(crate) enum CodebaseAutoIndexingSurface {
    Local,
    Remote,
}

impl CodebaseAutoIndexingSurface {
    fn required_feature_enabled(self) -> bool {
        match self {
            Self::Local => true,
            // Remote indexing requires Warp's hosted embedding/search backend, which Zap does
            // not carry. Keep the status surface but never issue remote index requests.
            Self::Remote => false,
        }
    }
}

pub(crate) fn should_auto_index_codebase(surface: CodebaseAutoIndexingSurface) -> bool {
    // Zap does not carry Warp's hosted embedding/search backend. Keep the local and remote
    // status/gating shell, but do not start an index that cannot be queried.
    codebase_auto_indexing_enabled(surface, false, false)
}

fn codebase_indexing_enabled(
    surface: CodebaseAutoIndexingSurface,
    codebase_context_enabled: bool,
) -> bool {
    FeatureFlag::FullSourceCodeEmbedding.is_enabled()
        && surface.required_feature_enabled()
        && codebase_context_enabled
}

pub(crate) fn codebase_auto_indexing_enabled(
    surface: CodebaseAutoIndexingSurface,
    codebase_context_enabled: bool,
    auto_indexing_enabled: bool,
) -> bool {
    codebase_indexing_enabled(surface, codebase_context_enabled) && auto_indexing_enabled
}

pub(crate) fn auto_index_candidate_roots<Root>(
    roots: impl IntoIterator<Item = Root>,
    mut should_request_index: impl FnMut(&Root) -> bool,
) -> Vec<Root>
where
    Root: Clone + Eq + Hash,
{
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for root in roots {
        if seen.insert(root.clone()) && should_request_index(&root) {
            candidates.push(root);
        }
    }
    candidates
}

#[cfg(test)]
#[path = "codebase_auto_indexing_tests.rs"]
mod tests;
