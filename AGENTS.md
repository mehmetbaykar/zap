# AGENTS.md

> This file is a navigation document for AI/automation agents working in this repository. It summarizes the repository's overall architecture, the responsibility of each crate in the Cargo workspace, the boundaries of each submodule under the `app/` main binary, and the engineering conventions that must be followed before making changes.
>
> It is a companion to `WARP.md`: `WARP.md` is the engineer's handbook (commands, style, process), and this file is the **code map**. Read `WARP.md` first, then use this file to locate the correct crate / module.

---

## 1. Repository overview

Warp is a primarily-Rust **agentic terminal / development environment**: on top of an in-house UI framework (WarpUI), it integrates terminal emulation, an AI Agent, cloud sync (Drive), code review, completion, Notebook, settings, IPC, and more.

Top-level directories:

| Directory | Purpose |
|------|------|
| `app/` | The main binary crate (`warp`); wires up all subsystems, the UI, database migrations, and the platform glue layer |
| `crates/` | 67 workspace members, library crates split by responsibility |
| `command-signatures-v2/` | A standalone subproject (`--exclude`d when running nextest) |
| `script/` | Cross-platform bootstrap, build, and presubmit scripts |
| `resources/` | Runtime resources such as fonts, icons, shell integration scripts, and shaders |
| `docker/` | Containerized-build related |
| `specs/` | Product/technical spec documents |
| `.agents/skills`, `.claude/skills` | Skill descriptions for agent workflows (create PR, fix errors, feature gradual rollout, etc.) |
| `.warp/`, `.config/`, `.cargo/`, `.vscode/` | Various tool configurations |

Build system: a Cargo workspace, `resolver = "2"`, with `default-members` deliberately narrowed to the subset that frequently needs compiling/testing (see `Cargo.toml`). `serve-wasm` and `integration` are not in `default-members` by default.

License split:
- `crates/warpui` and `crates/warpui_core` → MIT
- everything else → AGPL-3.0-only

---

## 2. Top-level architecture layers

From the bottom up there are roughly 4 layers. When adding code or locating a bug, first determine which layer the change belongs to, and **do not invert dependencies across layers**.

```
app/  (main binary: assembly, entry points, platform glue, persistence migrations, UI view root)
  ↑
product-domain crates: ai / computer_use / vim / onboarding /
              warp_completer / lsp / languages / code-review …
  ↑
framework crates: warpui / warpui_core / warpui_extras / editor /
            ui_components / sum_tree / syntax_tree
  ↑
infrastructure crates: warp_core / warp_util / http_client /
                websocket / ipc / jsonrpc / persistence / graphql /
                managed_secrets / virtual_fs / watcher / asset_cache …
```

Key architectural patterns (see `WARP.md` for details):

1. **Entity-Handle system**: `App` globally owns all view/model entities; views reference each other via `ViewHandle<T>` rather than owning them directly.
2. **Element / Action**: the UI is composed of a declarative Element tree + an Action event system (Flutter-style).
3. **Cross-platform**: native implementations for macOS / Windows / Linux + a WASM target; platform code is isolated with `#[cfg(...)]`.
4. **AI integration**: Agent Mode and context indexing; the code is concentrated in `app/src/ai` (389 files) and `crates/ai`.
5. **Cloud sync**: `Drive` keeps objects in sync across multiple devices; see `app/src/drive` and `crates/warp_files`.
6. **Feature Flag**: runtime gradual rollout is preferred over `#[cfg]`; the enum is defined in `crates/warp_core/src/features.rs`.

---

## 3. `crates/` at a glance

The table below lists all 67 crates grouped by topic. Each row gives only a **one-sentence responsibility**; for implementation details, open the corresponding `crates/<name>/src/lib.rs` directly (many crates have `//!` module docs at the top of `lib.rs`).

### 3.1 UI framework / view layer

| Crate | Responsibility |
|-------|------|
| `warpui_core` | WarpUI framework core (MIT): infrastructure such as `App` / `Entity` / `ViewHandle` / `AppContext` |
| `warpui` | WarpUI higher-level components, Element tree, layout, rendering pipeline (MIT) |
| `warpui_extras` | Optional WarpUI extensions; not all features enabled by default |
| `ui_components` | Higher-level component library reused across views (buttons, inputs, lists, modals, etc.) |
| `editor` (`warp_editor`) | Text editor: buffer, selection, cursor, key mapping, undo stack |
| `sum_tree` | A persistent balanced B-tree, the core data structure for the editor / Notebook / large lists |
| `syntax_tree` | Tree-sitter wrapper and syntax-highlighting support |
| `markdown_parser` | Markdown parsing (used by AI messages, document views, Notebook, etc.) |
| `vim` | Vim-mode key bindings and operation semantics |
| `voice_input` | Voice input support |

### 3.2 Terminal

| Crate | Responsibility |
|-------|------|
| `warp_terminal` | Terminal emulation core: PTY management, ANSI/VT parsing, grid, scrolling, shell integration hooks |
| `input_classifier` | Classifies the intent of terminal input (pure command / natural language / AI Prompt) |
| `natural_language_detection` | Natural-language detection (works with `input_classifier`) |

### 3.3 AI / Agent

| Crate | Responsibility |
|-------|------|
| `ai` | AI model client, Prompt orchestration, Agent protocol, tool-calling framework |
| `computer_use` | The Rust-side implementation of "Computer Use" tool capabilities (screenshot, click, type, etc.) |
| `command-signatures-v2` | Command signatures v2 (command-classification metadata for the AI); a standalone project, not part of the main workspace test set |
| `onboarding` | New-user onboarding flow data/state |

### 3.4 Networking / protocol / IPC

| Crate | Responsibility |
|-------|------|
| `http_client` | The workspace's unified HTTP client wrapper |
| `http_server` | An embedded HTTP server (local RPC, login callbacks, etc.) |
| `websocket` | A WebSocket abstraction shared by native and WASM, adapting `graphql_ws_client` |
| `ipc` | A generic typed IPC request/response protocol (inter-process) |
| `jsonrpc` | JSON-RPC implementation |
| `lsp` | Language Server Protocol client implementation |
| `remote_server` | The server-side logic for the remote sshd mode |
| `serve-wasm` | A helper server that hosts the WASM build artifacts (not compiled by default) |
| `firebase` | Firebase client utilities (Crash/analytics channels, etc.) |

### 3.5 Persistence / files / resources

| Crate | Responsibility |
|-------|------|
| `persistence` | The Diesel + SQLite persistence-layer foundation; **migrations live in `app/migrations/`, and the schema in `app/src/persistence/schema.rs`** |
| `warp_files` | Syncable file objects such as Drive files, Workflows, and Notebooks |
| `virtual_fs` | An abstract filesystem (the test mock and the production real FS share an interface) |
| `repo_metadata` | Repository metadata: file-tree construction, `.gitignore` handling, filesystem watching |
| `watcher` | A filesystem watcher (a wrapper around `notify`) |
| `asset_cache` | Disk/memory caching for resources |
| `asset_macro` | Resource-reference macros such as `bundled!` / `theme!` |
| `managed_secrets` / `managed_secrets_wasm` | Keychain / DPAPI / Linux Keyring abstraction + WASM proxy |

### 3.6 Configuration / settings

| Crate | Responsibility |
|-------|------|
| `settings` | Settings storage and change distribution |
| `settings_value` | The `SettingsValue` trait: controls TOML serialization semantics |
| `settings_value_derive` | The `#[derive(SettingsValue)]` procedural macro (converts enum variants to snake_case, etc.) |
| `warp_features` | The higher-level feature-flag API (consumer side) |
| `channel_versions` | Release channels (stable/preview/dogfood) and version comparison |

### 3.7 Commands / completion / languages

| Crate | Responsibility |
|-------|------|
| `command` | A safe wrapper for cross-platform process spawning, **with special handling for Windows' `no_window` flag**; all newly-spawned child processes go through here |
| `warp_completer` | The completion engine (supports `--features v2`) |
| `languages` | Registration of languages/extensions/Tree-sitter grammars |
| `warp_ripgrep` | A thin ripgrep wrapper for use by `warp_cli` |
| `warp_cli` | In-binary CLI subcommand parsing (`warp <subcmd>`) |
| `fuzzy_match` | Fuzzy matching + glob-style wildcards, used for path search and the command palette |

### 3.8 Platform / system services

| Crate | Responsibility |
|-------|------|
| `app-installation-detection` | Detects apps already installed on the system (for launcher integration) |
| `prevent_sleep` | Suppresses sleep (during long tasks / an AI Agent) |
| `isolation_platform` | A compatibility layer for running inside sandboxes such as Docker / GitHub Actions |
| `node_runtime` | Automatically installs/manages Node.js and npm (macOS/Linux/Windows × multiple architectures) |
| `warp_js` | A helper abstraction for manipulating JavaScript values/functions from the Rust side |

### 3.9 Common utilities / communication

| Crate | Responsibility |
|-------|------|
| `warp_core` | The lowest-level "core" in the workspace: platform abstraction, and the `FeatureFlag` enum plus `DOGFOOD/PREVIEW/RELEASE_FLAGS` in `features.rs` |
| `warp_util` | Common utility functions reused across multiple crates |
| `warp_logging` | The unified entry point for logging configuration |
| `simple_logger` | A simple async file logger for stderr-only processes such as `remote_server` |
| `warp_web_event_bus` | A web-side event bus (for the embedded web view) |
| `field_mask` | A gRPC/Proto-style FieldMask utility |
| `string-offset` | Base offset types (byte/char/utf16) |
| `handlebars` | A Handlebars template-engine wrapper |
| `integration` | The integration-test framework; for testing only |

> Naming gotcha: the package name of `crates/editor` is `warp_editor`; `crates/isolation_platform` is `warp_isolation_platform`; `crates/managed_secrets` is `warp_managed_secrets`; `crates/virtual_fs` is `virtual-fs` (with a hyphen); and `crates/string-offset` is `string-offset` (with a hyphen).

---

## 4. `app/` submodule navigation

Under `app/src/` there are 60+ flatly-laid-out product-domain directories, each roughly corresponding to a single product feature line. The following are grouped by topic; the number in parentheses is the approximate `.rs` file count, used to estimate module size:

### 4.1 Startup / assembly / global
- `bin/` (7) — multiple binary entry points (the main program and bundled tools).
- `lib.rs` / `app_state.rs` / `app_state_tests.rs` — the application state root.
- `app_menus.rs`, `app_services/`, `app_id_test.rs`
- `appearance.rs`, `gpu_state.rs`, `font_fallback.rs`, `global_resource_handles.rs`
- `dynamic_libraries.rs`, `alloc.rs`, `tracing.rs`, `profiling.rs`
- `crash_recovery.rs`, `crash_reporting/` (4)
- `features.rs` — the consumption of `warp_core::FeatureFlag` within `app/`; when adding a flag you usually need to wire it up in both places.
- `channel.rs`, `download_method.rs`, `autoupdate/` (8)

### 4.2 Terminal
- `terminal/` (427) — the main body: shell processes, PTY, grid, blocks, shell integration, command execution, I/O pipeline.
- `default_terminal/` (2) — the default-terminal startup logic.
- `shell_indicator.rs`, `prefix.rs` / `prefix_test.rs` (command-prefix parsing), `vim_registers.rs`

### 4.3 AI / Agent
- `ai/` (389) — contains the Agent UI, conversation model, Agent management, tools/MCP, Cloud Agent, Plan/Diff views, artifacts, blocklist, execution profiles, etc. **This is the largest subtree in the repository**; before making changes, grep within this directory for the specific subtopic (`agent_*`, `conversation_*`, `cloud_agent_*`, `mcp`, `tool_*`).
- `ai_assistant/` (9) — the legacy AI-assistance entry point/adapter.
- `chip_configurator/`, `context_chips/` (22) — Agent context-chip selection/construction.
- `coding_entrypoints/` (5), `coding_panel_enablement_state.rs`
- `prompt/` (2), `tips/` (3), `voice/` (2), `completer/` (3)

### 4.4 Editor / code / Review
- `editor/` (38) — the main editor integration.
- `code/` (52) — code views, diff, navigation.
- `code_review/` (36) — the Code Review flow.
- `notebooks/` (30), `workflows/` (22)

### 4.5 Search
- `search/` (172) — multi-target search (files, commands, Agent history, etc.).
- `search_bar.rs`

### 4.6 Server communication / Drive / sync
- `server/` (55) — HTTP/WS interaction with the warp backend (corresponds to the local dev mode `with_local_server`).
- `drive/` (45) — the entry point for cloud object sync.
- `cloud_object/` (12) — the cloud-object abstraction layer (workflow, notebook, etc.).
- `remote_server/` (5) — the client-side glue for connecting to the remote-mode sshd.

### 4.7 Settings / user config / themes / Onboarding
- `settings/` (46), `settings_view/` (63)
- `user_config/` (6), `themes/` (11), `appearance.rs`
- `experiments/` (7), `tab_configs/` (15), `launch_configs/` (4)
- `tips/`, `banner/` (3), `quit_warning/` (1), `wasm_nux_dialog.rs`, `referral_theme_status.rs`

### 4.8 Authentication / billing / usage
- `auth/` (22) — login, token, SSO.
- `billing/` (3), `pricing/` (1), `usage/` (1), `reward_view.rs`

### 4.9 Persistence
- `persistence/` (9) — Diesel migrations assembly, `schema.rs` (generated by Diesel), and the migration runner.
- Migration files live in the top-level `migrations/` directory of the repository (managed by the Diesel CLI).

### 4.10 Platform / system integration
- `platform/` (2), `system/` (3) / `system.rs`
- `login_item/` (3), `antivirus/` (3), `network.rs`
- `external_secrets/` (1), `env_vars/` (14)
- `keyboard.rs` / `keyboard_test.rs`, `safe_triangle.rs` / `safe_triangle_tests.rs` (the menu-hover safe triangle)

### 4.11 View root / panels / common UI
- `root_view.rs` / `root_view_tests.rs`
- `pane_group/` (35) — split-pane/block layout.
- `tab.rs`, `command_palette.rs`, `modal.rs`, `menu.rs` / `menu_test.rs`
- `palette.rs`, `notification.rs`, `resource_center/` (10)
- `view_components/` (20), `ui_components/` (14)
- `workspace/` (54), `workspaces/` (10), `voltron.rs` (multi-window / multi-workspace coordination)
- `session_management.rs`, `undo_close/` (3), `word_block_editor.rs`
- `suggestions/` (2), `input_suggestions.rs` / `input_suggestions_test.rs`
- `plugin/` (21) — plugin system integration.
- `uri/` (7) — `warp://` URL handling.
- `debug_dump.rs`, `debounce.rs`, `interval_timer.rs`, `throttle.rs`
- `linear.rs`, `resource_limits.rs`, `warp_managed_paths_watcher.rs`
- `preview_config_migration.rs` / `preview_config_migration_tests.rs`
- `window_settings.rs`, `projects.rs`

### 4.12 Test infrastructure
- `integration_testing/` (79) — end-to-end integration-test support.
- `test_util/` (6) — common unit-test utilities.

---

## 5. Engineering discipline (hard constraints for the Agent)

> These are compiled from `WARP.md` and the project's custom rules; this file's verification requirement for the agent is `cargo check`.

### 5.1 Must-read conventions
- **Comments/replies must always use Simplified Chinese** (user rule).
- For searching/grepping within the git index, use the `fff` tool or `rg -n "<keyword>" <path>`; `read_file` is only for images/binaries.
- Before opening a PR / pushing a new commit, you **only** need to pass: `cargo check`.
- Changes must be precise: **every modified line must trace back to a user request**; do not casually "improve" unrelated code, comments, or formatting.
- Prefer simplicity: do not introduce abstractions, configuration, error handling, or extra features for a single use site.
- Explain options and expose uncertainty rather than silently making choices on the user's behalf.
- worktree path: .worktrees/<worktree_name>/

### 5.2 Rust style (excerpted from `WARP.md`)
- Do not write redundant type annotations on closure parameters.
- Consolidate `use` at the top; do not write long path qualifiers; the exception is inside `#[cfg]` branches.
- Name the context parameter `ctx` and put it last; if there is also a closure parameter, put the closure last.
- For unused parameters, **delete them directly** rather than adding a `_` prefix, and update the call sites accordingly.
- Macros such as `println!` / `format!` should use inline format arguments (`"{x}"` rather than `"{}", x`) to satisfy `uninlined_format_args`.
- `match` statements **must not use the `_` wildcard** (unless truly necessary); keep matches exhaustive.
- Do not delete/change existing comments because of an unrelated modification.

### 5.3 Terminal model lock (high priority!)
- Calling `TerminalModel::lock()` deadlocks very easily (on macOS this shows up as a frozen UI / spinning beachball).
- Before adding a `model.lock()`, you must confirm that no caller higher up the stack already holds the lock; where possible, pass the already-locked reference down the call stack rather than locking again.
- Minimize the locked scope, and do not call functions that might lock again while holding the lock.

### 5.4 Feature Flag
- Adding: add a variant to the `FeatureFlag` enum in `crates/warp_core/src/features.rs`; add it to `DOGFOOD_FLAGS` / `PREVIEW_FLAGS` / `RELEASE_FLAGS` as needed.
- Using: **prefer** the runtime `FeatureFlag::Xxx.is_enabled()` over `#[cfg(...)]`; only use `cfg` when it would not compile without it (platform / optional dependency).
- Wrap an entire product feature rather than adding it at every call site; once it is stably shipped, **clean up the flag and the dead branches**.
- The UI entry point and the code path must use the same flag.

### 5.5 Database
- ORM: Diesel + SQLite.
- Adding/changing the schema must go through a migration: add a new directory under `migrations/` (`up.sql` / `down.sql`); do not hand-edit `app/src/persistence/schema.rs` (generated by `diesel print-schema`).

### 5.6 Testing
- Use `cargo nextest run --no-fail-fast --workspace --exclude command-signatures-v2`.
- Put unit tests in `${filename}_tests.rs` or `mod_test.rs`, and at the end of the original file use:

  ```rust
  #[cfg(test)]
  #[path = "filename_tests.rs"]
  mod tests;
  ```

- For integration tests use the `crates/integration` framework; examples are in `app/src/integration_testing/`.

### 5.7 Cross-process commands
- Do not use `std::process::Command::new(...)` directly (on Windows in particular it pops up a window); always go through `crates/command`.

### 5.8 Subagents / multi-agent
- Split a large task into subtasks with **non-overlapping write domains** and dispatch them in parallel; information-gathering tasks can run in parallel.
- Do simple tasks directly; do not over-split them.

---

## 6. Common entry-point quick reference

| What you want to do | Starting point |
|---------|------|
| Change terminal grid / shell integration | `crates/warp_terminal/src/`, in tandem with `app/src/terminal/` |
| Change Agent UI / conversation | grep by topic within `app/src/ai/` using `agent_*` / `conversation_*` |
| Change command completion | `crates/warp_completer/` (note `--features v2`) |
| Change AI model / tool-calling protocol | `crates/ai/` |
| Add a new setting | `crates/settings_value*`, `crates/settings`; the UI is in `app/src/settings_view/` |
| Add a Feature Flag | `crates/warp_core/src/features.rs` + the use sites |
| Change a cloud sync object | `crates/warp_files` + `app/src/drive/` + `app/src/cloud_object/` |
| Change the persistence schema | add a migration under `migrations/` + `crates/persistence` |
| Add a new binary tool | `app/src/bin/` |
| Platform-specific code | use `#[cfg(target_os = "...")]`; the UI platform glue is in `app/src/platform/` |
| Vim mode | `crates/vim` + `app/src/vim_registers.rs` |
| Notebook / Workflow | `app/src/notebooks/`, `app/src/workflows/`, `crates/warp_files` |
| Cross-platform process spawning | `crates/command` |
| File search / watching | `crates/repo_metadata`, `crates/watcher`, `crates/warp_ripgrep` |

---

## 7. Pre-change checklist

Before touching the keyboard to change code, ask yourself once:

1. Which layer / which crate / which `app/src/<submodule>` does this belong to? Will the change cross a layer boundary?
2. Does it need a new dependency? If an existing workspace dependency can be reused, prefer reusing `Cargo.toml` `[workspace.dependencies]`.
3. Is this a product feature? Does it need to be wrapped in a Feature Flag?
4. Does it touch the terminal model? Does the current call stack already hold the `TerminalModel` lock?
5. Does it touch a child process? Did it go through `crates/command`?
6. Does it touch persistence? Does it need a migration?
7. Have you written the corresponding `${file}_tests.rs`?
8. Is `cargo check` green?
9. Can every modified line be mapped one-to-one to a user request? Should any casual "small refactor" be reverted?

Go through all 9 items above, then deliver.
