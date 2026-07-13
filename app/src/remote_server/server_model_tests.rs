use std::collections::HashMap;

use std::fs;

use crate::code_review::diff_state::DiffMode;
use crate::remote_server::diff_state_tracker::DiffModelKey;
use warp_util::standardized_path::StandardizedPath;
use warpui::App;

use super::super::diff_state_tracker::RemoteDiffStateManager;
use super::super::proto::{
    list_directory_response, read_file_chunk_response, resolve_path_response, server_message,
    write_file_chunk_response, Authenticate, CreateDirectory, Initialize, ListDirectory,
    ReadFileChunk, ResolvePath, WriteFileChunk,
};
use super::super::protocol::RequestId;
#[cfg(feature = "local_fs")]
use super::super::server_buffer_tracker::ServerBufferTracker;
use super::{PendingFileOps, ServerModel};

fn test_model(app: &mut App) -> ServerModel {
    ServerModel {
        connection_senders: HashMap::new(),
        snapshot_sent_roots_by_connection: HashMap::new(),
        grace_timer_cancel: None,
        in_progress: HashMap::new(),
        host_id: "test-host-id".to_string(),
        executors: HashMap::new(),
        pending_file_ops: PendingFileOps::new(),
        #[cfg(feature = "local_fs")]
        buffers: ServerBufferTracker::new(),
        auth_token: None,
        diff_states: app.add_model(|_| RemoteDiffStateManager::new()),
    }
}

/// Uses `try_new` instead of `try_from_local` so that Unix-style paths
/// like `/repo` are recognised as absolute on all platforms (including Windows).
fn test_key(repo: &str, mode: DiffMode) -> DiffModelKey {
    DiffModelKey {
        repo_path: StandardizedPath::try_new(repo).unwrap(),
        mode,
    }
}

fn request_id() -> RequestId {
    RequestId::from("test-request".to_string())
}

#[test]
fn fresh_model_starts_without_auth_token() {
    App::test((), |mut app| async move {
        let model = test_model(&mut app);

        assert_eq!(model.auth_token(), None);
    });
}

#[test]
fn initialize_with_auth_token_stores_token() {
    App::test((), |mut app| async move {
        let mut model = test_model(&mut app);

        model.handle_initialize(
            Initialize {
                auth_token: "initial-token".to_string(),
                codebase_index_limits: None,
            },
            &request_id(),
        );

        assert_eq!(model.auth_token(), Some("initial-token"));
    });
}

#[test]
fn empty_initialize_preserves_existing_auth_token() {
    App::test((), |mut app| async move {
        let mut model = test_model(&mut app);
        model.handle_initialize(
            Initialize {
                auth_token: "initial-token".to_string(),
                codebase_index_limits: None,
            },
            &request_id(),
        );

        model.handle_initialize(
            Initialize {
                auth_token: String::new(),
                codebase_index_limits: None,
            },
            &request_id(),
        );

        assert_eq!(model.auth_token(), Some("initial-token"));
    });
}

#[test]
fn authenticate_with_auth_token_replaces_auth_token() {
    App::test((), |mut app| async move {
        let mut model = test_model(&mut app);
        model.handle_initialize(
            Initialize {
                auth_token: "initial-token".to_string(),
                codebase_index_limits: None,
            },
            &request_id(),
        );

        model.handle_authenticate(Authenticate {
            auth_token: "rotated-token".to_string(),
        });

        assert_eq!(model.auth_token(), Some("rotated-token"));
    });
}

#[test]
fn empty_authenticate_preserves_existing_auth_token() {
    App::test((), |mut app| async move {
        let mut model = test_model(&mut app);
        model.handle_initialize(
            Initialize {
                auth_token: "initial-token".to_string(),
                codebase_index_limits: None,
            },
            &request_id(),
        );

        model.handle_authenticate(Authenticate {
            auth_token: String::new(),
        });

        assert_eq!(model.auth_token(), Some("initial-token"));
    });
}

// ── Diff state: connection cleanup ──────────────────────────────────

#[test]
fn deregister_connection_cleans_up_diff_state_subscriptions() {
    App::test((), |mut app| async move {
        let mut model = test_model(&mut app);
        let conn = uuid::Uuid::new_v4();

        // Register the connection.
        let (tx, _rx) = async_channel::unbounded();
        model.connection_senders.insert(conn, tx);

        // Subscribe the connection to diff state via the manager.
        let key = test_key("/repo", DiffMode::Head);
        let key2 = key.clone();
        let key3 = key.clone();
        model.diff_states.update(&mut app, |mgr, _ctx| {
            mgr.subscribe_connection(key, conn);
        });
        let has_sub = model.diff_states.read(&app, |mgr, _ctx| {
            !mgr.subscribed_connections(&key2).is_empty()
        });
        assert!(has_sub);

        // Simulate deregister_connection's diff state cleanup.
        model.diff_states.update(&mut app, |mgr, _ctx| {
            mgr.remove_connection(conn);
        });
        let has_sub = model.diff_states.read(&app, |mgr, _ctx| {
            !mgr.subscribed_connections(&key3).is_empty()
        });
        assert!(!has_sub);
    });
}

#[test]
fn diff_states_starts_empty() {
    App::test((), |mut app| async move {
        let model = test_model(&mut app);
        let key = test_key("/repo", DiffMode::Head);
        let empty = model.diff_states.read(&app, |mgr, _ctx| {
            mgr.subscribed_connections(&key).is_empty()
        });
        assert!(empty);
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn resolve_path_reports_file_metadata() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("note.txt");
        fs::write(&file_path, "hello").unwrap();
        let model = test_model(&mut app);

        let response = model.handle_resolve_path(ResolvePath {
            path: file_path.to_string_lossy().to_string(),
        });

        let server_message::Message::ResolvePathResponse(response) = response.into_message() else {
            panic!("expected ResolvePathResponse");
        };
        let Some(resolve_path_response::Result::Success(success)) = response.result else {
            panic!("expected resolve path success");
        };
        assert_eq!(
            success.canonical_path,
            fs::canonicalize(&file_path).unwrap().to_string_lossy()
        );
        assert_eq!(
            success.kind,
            super::super::proto::FileSystemEntryKind::File as i32
        );
        assert_eq!(success.size_bytes, Some(5));
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn list_directory_returns_sorted_metadata() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.txt"), "b").unwrap();
        fs::create_dir(dir.path().join("a-dir")).unwrap();
        let model = test_model(&mut app);

        let response = model.handle_list_directory(ListDirectory {
            path: dir.path().to_string_lossy().to_string(),
        });

        let server_message::Message::ListDirectoryResponse(response) = response.into_message()
        else {
            panic!("expected ListDirectoryResponse");
        };
        let Some(list_directory_response::Result::Success(success)) = response.result else {
            panic!("expected list directory success");
        };
        assert_eq!(
            success.canonical_path,
            fs::canonicalize(dir.path()).unwrap().to_string_lossy()
        );
        assert_eq!(success.entries.len(), 2);
        assert_eq!(success.entries[0].name, "a-dir");
        assert_eq!(
            success.entries[0].kind,
            super::super::proto::FileSystemEntryKind::Directory as i32
        );
        assert_eq!(success.entries[1].name, "b.txt");
        assert_eq!(
            success.entries[1].kind,
            super::super::proto::FileSystemEntryKind::File as i32
        );
        assert_eq!(success.entries[1].size_bytes, Some(1));
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn read_and_write_file_chunks_round_trip_binary_data() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("blob.bin");
        let model = test_model(&mut app);

        let write_response = model.handle_write_file_chunk(WriteFileChunk {
            path: file_path.to_string_lossy().to_string(),
            offset: 0,
            bytes: vec![0, 1, 2, 3],
            truncate: true,
            executable: None,
        });
        let server_message::Message::WriteFileChunkResponse(write_response) =
            write_response.into_message()
        else {
            panic!("expected WriteFileChunkResponse");
        };
        let Some(write_file_chunk_response::Result::Success(write_success)) = write_response.result
        else {
            panic!("expected write chunk success");
        };
        assert_eq!(write_success.next_offset, 4);

        let read_response = model.handle_read_file_chunk(ReadFileChunk {
            path: file_path.to_string_lossy().to_string(),
            offset: 1,
            max_bytes: 2,
        });
        let server_message::Message::ReadFileChunkResponse(read_response) =
            read_response.into_message()
        else {
            panic!("expected ReadFileChunkResponse");
        };
        let Some(read_file_chunk_response::Result::Success(read_success)) = read_response.result
        else {
            panic!("expected read chunk success");
        };
        assert_eq!(read_success.bytes, vec![1, 2]);
        assert_eq!(read_success.next_offset, 3);
        assert_eq!(read_success.total_size, Some(4));
        assert!(!read_success.eof);
    });
}

#[cfg(feature = "local_fs")]
#[test]
fn create_directory_creates_nested_directories() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        let model = test_model(&mut app);

        let response = model.handle_create_directory(CreateDirectory {
            path: nested.to_string_lossy().to_string(),
        });

        let server_message::Message::CreateDirectoryResponse(response) = response.into_message()
        else {
            panic!("expected CreateDirectoryResponse");
        };
        assert!(matches!(
            response.result,
            Some(super::super::proto::create_directory_response::Result::Success(_))
        ));
        assert!(nested.is_dir());
    });
}
