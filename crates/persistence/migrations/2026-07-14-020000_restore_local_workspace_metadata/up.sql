-- Restore local codebase-index and language-server state after the fork's
-- 2026-05-11 migration removed these tables with the cloud-backed implementation.
CREATE TABLE IF NOT EXISTS workspace_metadata (
    id integer NOT NULL PRIMARY KEY,
    repo_path TEXT NOT NULL UNIQUE,
    navigated_ts DATETIME,
    modified_ts DATETIME,
    queried_ts DATETIME
);

CREATE TABLE IF NOT EXISTS workspace_language_server (
    id integer NOT NULL PRIMARY KEY,
    workspace_id integer NOT NULL,
    language_server_name TEXT NOT NULL,
    enabled TEXT NOT NULL,
    FOREIGN KEY (workspace_id) REFERENCES workspace_metadata (id)
);
