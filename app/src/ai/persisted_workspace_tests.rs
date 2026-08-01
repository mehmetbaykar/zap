use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{PersistedWorkspace, Workspace, WorkspaceMetadata};

fn empty_persisted_workspace() -> PersistedWorkspace {
    PersistedWorkspace {
        workspaces: HashMap::new(),
        #[cfg(feature = "local_fs")]
        lsp_installation_status: HashMap::new(),
    }
}

fn insert_workspace(persisted_workspace: &mut PersistedWorkspace, path: &Path) {
    persisted_workspace.workspaces.insert(
        path.to_path_buf(),
        Workspace {
            metadata: WorkspaceMetadata {
                path: path.to_path_buf(),
                navigated_ts: None,
                modified_ts: None,
                queried_ts: None,
            },
            language_servers: HashMap::new(),
        },
    );
}

#[test]
fn root_for_workspace_returns_none_when_unregistered() {
    let persisted_workspace = empty_persisted_workspace();
    let repository = PathBuf::from("/tmp/some-fresh-repo");

    assert!(
        persisted_workspace
            .root_for_workspace(&repository)
            .is_none()
    );
}

#[test]
fn root_for_workspace_resolves_registered_ancestor() {
    let mut persisted_workspace = empty_persisted_workspace();
    let repository = PathBuf::from("/tmp/registered-repo");
    insert_workspace(&mut persisted_workspace, &repository);

    let nested_path = repository.join("src/foo/bar.rs");
    assert_eq!(
        persisted_workspace.root_for_workspace(&nested_path),
        Some(repository.as_path())
    );
}

#[test]
fn root_for_workspace_ignores_unrelated_registered_workspace() {
    let mut persisted_workspace = empty_persisted_workspace();
    insert_workspace(
        &mut persisted_workspace,
        &PathBuf::from("/tmp/some-other-repo"),
    );

    let unrelated_path = PathBuf::from("/tmp/unrelated-repo/src/main.rs");
    assert!(
        persisted_workspace
            .root_for_workspace(&unrelated_path)
            .is_none()
    );
}
