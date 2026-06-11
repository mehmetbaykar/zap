# SFTP File Manager Design Document

**Date**: 2026-05-26
**Status**: Approved

## Overview

Add a native SFTP file manager feature to the Zap terminal, using the `ssh2` crate (libssh2 bindings) to implement the SFTP protocol, providing complete remote file browsing, transfer, and management capabilities. Implemented as a standalone Pane panel, coexisting with the existing Server File Browser, with no need to install a remote daemon.

## Technical Approach

Use the `ssh2` crate to implement the SFTP protocol directly; it has been verified stable in similar projects and has complete SFTP functionality (directory traversal, streaming transfer, permission management).

Dependencies: `ssh2` (libssh2 bindings), `smol` (async runtime), `thiserror` (error handling). On Windows, enable the `openssl-on-win32` feature and vendored openssl-sys.

## Crate Structure and Module Organization

### Protocol layer — `crates/warp_sftp/` (new crate)

```
crates/warp_sftp/
  Cargo.toml
  build.rs                          # Windows links advapi32
  src/
    lib.rs                          # Module root, exports public API
    error.rs                        # SftpError / SftpChannelError
    types.rs                        # FileType / Metadata / DirEntry / OpenOptions, etc.
    session.rs                      # SftpSession (SSH connection management, authentication)
    sftp.rs                         # Sftp (SFTP channel, file/directory operations)
    dir.rs                          # Dir (directory reading and sorting)
    file.rs                         # File (file read/write)
```

### UI layer — `app/src/sftp_manager/` (new module)

```
app/src/sftp_manager/
  mod.rs                            # Module root
  types.rs                          # UI types: FileEntry / TransferTask / Dialog / ConnectionState
  sftp_ops.rs                       # High-level operation bridge
  browser.rs                        # SftpBrowserView main view
  file_list.rs                      # File list rendering
  breadcrumb.rs                     # Breadcrumb navigation
  context_menu.rs                   # Right-click menu
  dialogs.rs                        # Dialogs
  transfer_panel.rs                 # Transfer progress panel
```

### Pane integration

```
app/src/pane_group/pane/sftp_pane.rs (new)
```

## Core Protocol Layer Design

### session.rs — connection management

- `SftpSession`: internally holds `Arc<ssh2::Session>` + `TcpStream`
- `connect(host, port, username, auth_method) -> Result<SftpSession>`: establish TCP connection → SSH handshake → authentication
- `sftp() -> Result<Sftp>`: open the SFTP subsystem on the existing session
- `disconnect()`: actively disconnect
- `Drop` automatically disconnects the connection

`AuthMethod` enum: `Password(String)` | `PublicKey { path, passphrase }`

### sftp.rs — SFTP channel operations

- `Sftp`: wraps `Arc<Mutex<ssh2::Sftp>>`, Clone + thread-safe
- Operations: `open`, `create_dir`, `remove_dir`, `remove_file`, `rename`, `stat`, `lstat`, `read_dir`, `symlink`, `readlink`

### dir.rs — directory reading

- `Dir::read_dir() -> Result<Vec<DirEntry>>`
- Filters out `.` and `..`, converts to DirEntry
- Sorting: directories first, then alphabetical order

### file.rs — file read/write

- `File`: wraps `ssh2::File`
- Operations: `read_to_end`, `write_all`, `read` (32KB chunks), `write` (32KB chunks), `flush`, `stat`

### types.rs — core types

- `FileType`: Dir | File | Symlink | Other
- `FilePermissions`: 9-bit Unix permissions (rwxrwxrwx)
- `Metadata`: type, perms, size, uid, gid, atime, mtime
- `DirEntry`: name, path, metadata
- `OpenOptions`: read, write, append, create, truncate
- `WriteMode`: Overwrite | Append | Resume

### error.rs — error types

- `SftpError`: IO | SSH2 | ConnectionFailed | AuthFailed | Timeout | NoSuchFile | PermissionDenied | General
- `SftpChannelError`: Sftp | SendFailed | RecvFailed

## UI Layer Design

### browser.rs — SftpBrowserView main view

Implements the `BackingView` + `TypedActionView` + `View` trait.

**State**:

| Field | Type | Description |
|------|------|------|
| connection | ConnectionState | Connecting/Connected/Disconnected/Failed |
| _session | Option\<SftpSession\> | Keeps the TCP connection alive |
| sftp | Option\<Sftp\> | SFTP channel |
| current_path | String | Current directory path |
| entries | Vec\<FileEntry\> | File list of the current directory |
| selection | Option\<usize\> | Selected item index |
| nav_history | NavHistory | Forward/back history |
| transfers | Vec\<TransferTask\> | Transfer queue |
| dialog | Option\<Dialog\> | Current dialog state |
| search_filter | Option\<String\> | Search filter |

**Action enum**:

- Connect(node_id), Disconnect
- NavigateTo(path), GoBack, GoForward, GoUp, Refresh
- Upload, Download, Delete, Rename, CreateFolder
- Select(index), Open(index)
- ShowContextMenu(index)
- CancelTransfer(task_id)
- Search(filter)

**Render structure** (top to bottom):

1. Toolbar: back/forward/up/refresh buttons + upload button + new folder button
2. Breadcrumb navigation: clickable path segments
3. File list: tabular (name/size/modification date), click to select, double-click to open
4. Transfer panel: collapsible at the bottom, shows active transfer tasks and progress
5. Right-click menu: Open/Download/Rename/Delete/Details
6. Dialogs: delete confirmation, rename input, new folder input, file details display

### sftp_ops.rs — high-level operation bridge

- `connect_from_server(server_info, secret_store) -> Result<(SftpSession, Sftp)>`: read configuration from the SSH manager → obtain credentials → establish connection
- `list_dir(sftp, path) -> Result<Vec<FileEntry>>`
- `upload_file_streaming(sftp, local, remote, cancel_flag)`: 32KB chunks, AtomicBool supports cancellation
- `download_file_streaming(sftp, remote, local, cancel_flag)`: 32KB chunks
- `upload_dir_recursive`, `download_dir_recursive`
- `delete_file`, `delete_dir_recursive`, `create_dir`, `rename`
- Concurrency control: AtomicUsize CAS limits to at most 2 parallel transfers

### Other UI modules

| Module | Responsibility |
|------|------|
| `file_list.rs` | File header + row rendering, directory/file icons, hover effect, selection highlight |
| `breadcrumb.rs` | Clickable segments from root to the current path, each segment triggers NavigateTo |
| `context_menu.rs` | Right-click menu items: Open/Download/Rename/Delete/Details |
| `dialogs.rs` | Modal dialog, EditorView text input, Enter to confirm / Escape to cancel |
| `transfer_panel.rs` | Transfer direction icon + file name + progress percentage + progress bar + status label |

### Keyboard shortcuts

| Key | Action |
|------|------|
| Backspace | Go back to the parent directory |
| Delete | Delete the selected item |
| Ctrl+Shift+N | New folder |
| Escape | Cancel search / close dialog |

## Integration and Entry Points

### Integration with the SSH manager

The SFTP browser obtains connection information through `warp_ssh_manager`.

Entry points:

- `app/src/ssh_manager/panel.rs`: add a "Browse SFTP" option to the server right-click menu
- `app/src/ssh_manager/server_view.rs`: add a "Browse SFTP" button to the server details action bar

Connection flow:

1. The user right-clicks a server in the SSH host list → menu item "Browse SFTP"
2. Obtain SshServerInfo (host, port, username, auth_type, key_path)
3. Obtain the password/key passphrase through KeychainSecretStore
4. Build the AuthMethod
5. SftpOps::connect_from_server() establishes the connection
6. Open the SftpBrowserView Pane and display the root directory

### Pane system integration

- `app/src/pane_group/pane/sftp_pane.rs` (new): `SftpPane` wraps `SftpBrowserView` as `PaneContent`
  - Implements the PaneContent trait
  - Snapshot serialized as `LeafContents::Sftp { node_id }`
  - Automatically reconnects based on node_id when restoring

Registration changes:

- `app/src/pane_group/pane/mod.rs`: declare the sftp_pane module
- `app/src/lib.rs`: register SftpPane with the View system

### Feature Flag

No Feature Flag is used; it is always globally available.

## Data Flow and Error Handling

### Operation data flow

```
User action (click/right-click/shortcut)
  → SftpBrowserView receives Action
  → dispatch_typed_action() matches the Action type
  → Async task submitted via ctx.spawn():
      ├── Obtain the SftpOps / Sftp instance
      ├── Execute the SFTP operation (runs on the smol thread pool)
      └── Return the result to the main thread
  → Update the SftpBrowserView state
  → Trigger a re-render
```

### Connection lifecycle

```
Open Pane → Connect(node_id)
  → Connecting status (shows loading animation)
  → Success → Connected (loads the root directory)
  → Failed → Failed (shows the error message + retry button)

Close Pane → Drop
  → SftpSession automatically disconnects (Drop impl)
```

### Error handling strategy

| Scenario | Handling |
|------|----------|
| Connection failed (network/authentication) | Display the error message, provide a retry button, no dialog |
| Directory load failed | Display an error prompt + refresh button in the file list area |
| File operation failed (delete/rename) | Inline error prompt (Toast style), does not block the UI |
| Transfer failed | The transfer panel is marked as Failed status, showing the error reason |
| Connection interrupted | Automatically switch to Disconnected status, prompt to reconnect |

All errors are uniformly mapped through `SftpError` into user-readable English prompts.
