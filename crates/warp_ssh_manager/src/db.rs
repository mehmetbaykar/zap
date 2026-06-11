//! A single process-wide `Mutex<SqliteConnection>` for the SSH manager.
//!
//! Status quo: openWarp's main write connection runs on a dedicated writer thread (see `app/src/persistence/sqlite.rs`)
//! and is handled asynchronously through a `ModelEvent` channel. Wiring the SSH manager into that event bus would require
//! adding 6+ enum variants plus cross-crate type exposure, which is too costly.
//!
//! Alternative: **SQLite WAL mode natively supports multiple write connections** (writes are mutually exclusive but retry with busy_timeout),
//! so we open a separate independent write connection here, keeping the behavior fully localized to this crate. The SSH manager's writes are
//! user-driven (creating/deleting nodes), happen very rarely, and their conflicts with the main writer thread are negligible.
//!
//! The path is passed in by the caller at initialization (`set_database_path`), so this crate does not directly depend on the app
//! layer's `database_file_path()`. When no path has been set, `with_conn` returns `Err(NotInitialized)`.

use anyhow::{anyhow, Result};
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static DB_PATH: OnceLock<PathBuf> = OnceLock::new();
static CONN: OnceLock<Mutex<SqliteConnection>> = OnceLock::new();

/// Called once at app startup with the sqlite db file path. Repeated calls are ignored
/// (OnceLock semantics).
pub fn set_database_path(path: PathBuf) {
    let _ = DB_PATH.set(path);
}

fn open() -> Result<SqliteConnection> {
    let path = DB_PATH
        .get()
        .ok_or_else(|| anyhow!("warp_ssh_manager::db: database path not initialized"))?;
    let url = path.to_string_lossy();
    let mut conn = SqliteConnection::establish(&url)?;
    conn.batch_execute(
        "PRAGMA foreign_keys = ON; \
         PRAGMA busy_timeout = 2000; \
         PRAGMA journal_mode = WAL;",
    )?;
    Ok(conn)
}

/// Run the closure while holding the lock. The connection is opened lazily on first call; later calls reuse it.
pub fn with_conn<R>(f: impl FnOnce(&mut SqliteConnection) -> Result<R>) -> Result<R> {
    let mtx = CONN.get_or_init(|| Mutex::new(open().expect("warp_ssh_manager db open")));
    let mut guard = mtx
        .lock()
        .map_err(|_| anyhow!("warp_ssh_manager db mutex poisoned"))?;
    f(&mut guard)
}
