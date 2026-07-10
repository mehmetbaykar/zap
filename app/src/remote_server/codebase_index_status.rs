use std::time::{SystemTime, UNIX_EPOCH};

use super::proto::{CodebaseIndexStatus, CodebaseIndexStatusState};

fn current_epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

pub(super) fn queued_codebase_index_status(repo_path: String) -> CodebaseIndexStatus {
    base_codebase_index_status(repo_path, CodebaseIndexStatusState::Queued)
}

pub(super) fn not_enabled_codebase_index_status(repo_path: String) -> CodebaseIndexStatus {
    base_codebase_index_status(repo_path, CodebaseIndexStatusState::NotEnabled)
}

pub(super) fn disabled_codebase_index_status(repo_path: String) -> CodebaseIndexStatus {
    base_codebase_index_status(repo_path, CodebaseIndexStatusState::Disabled)
}

fn base_codebase_index_status(
    repo_path: String,
    state: CodebaseIndexStatusState,
) -> CodebaseIndexStatus {
    CodebaseIndexStatus {
        repo_path,
        state: state.into(),
        last_updated_epoch_millis: Some(current_epoch_millis()),
        progress_completed: None,
        progress_total: None,
        failure_message: None,
        root_hash: None,
    }
}

// `codebase_index_status_to_proto` (and its `LocalCodebaseIndexStatus` /
// `CodebaseIndexFinishedStatus` / `SyncProgress` helpers) were removed here: they
// converted the state of `ai::index::full_source_code_embedding::manager`'s live
// indexing manager, whose `StoreClient` (see `ai::index::full_source_code_embedding::
// store_client`) only has a cloud-backed implementation (Warp's hosted embedding /
// rerank store via `server_api`/`warp_graphql`, both stripped from this fork). With
// no local indexing manager to report on, there is nothing left to convert; callers
// only ever need the static `queued`/`not_enabled`/`disabled` statuses above.
