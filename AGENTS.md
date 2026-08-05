# AGENTS.md

> This file is a navigation document for AI/automation agents working in this repository. It summarizes the repository's overall architecture, the responsibility of each crate in the Cargo workspace, the boundaries of each submodule under the `app/` main binary, and the engineering conventions that must be followed before making changes.
>
> It is a companion to `WARP.md`: `WARP.md` is the engineer's handbook (commands, style, process), and this file is the **code map**. Read `WARP.md` first, then use this file to locate the correct crate / module.

---

## 1. Repository overview

Zap is a **de-clouded fork of Warp** — a primarily-Rust agentic terminal / development environment: on top of an in-house UI framework (WarpUI), it integrates terminal emulation, an AI Agent (local BYOP providers instead of Warp's cloud), code review, completion, Notebook, settings, IPC, and more. Warp accounts, login, subscriptions, cloud sync, and telemetry are removed; legacy cloud code paths that remain (Drive, `server/`, `auth/`, `billing/`) are inert/stubbed.

Top-level directories:

| Directory | Purpose |
|------|------|
| `app/` | The main binary crate (`warp`); wires up all subsystems, the UI, database migrations, and the platform glue layer |
| `crates/` | 68 workspace members (+ `app`), library crates split by responsibility |
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
5. **Local persistence**: `Drive`-era object code (`app/src/drive`, `crates/warp_files`) persists locally only — the cloud sync transport is removed in this fork.
6. **Feature Flag**: runtime gradual rollout is preferred over `#[cfg]`; the enum is defined in `crates/warp_core/src/features.rs`.

---

## 3. `crates/` at a glance

The table below lists the crates grouped by topic. Each row gives only a **one-sentence responsibility**; for implementation details, open the corresponding `crates/<name>/src/lib.rs` directly (many crates have `//!` module docs at the top of `lib.rs`).

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

Fork identity & upstream sync state (2026-07-19):
- **Sync mechanism**: upstream `warpdotdev/warp` is tracked by incremental **git merges** (remote `upstream-warp`, always fetch `--no-tags`). The per-path conflict policy lives in `SPEC.md` (repo root, untracked-local — never `git add`); recurring keep-deleted paths in `script/upstream-strip.list`; register the merge driver once via `git config merge.openwarp-ours.driver true` (or `script/setup-merge-drivers.sh`). Merged through upstream **`0017f3059`** (2026-07-17). History: catch-up slices 1–7 (`c325d146a`→`19ebec9da`, 2026-07-11..16), then weekly waves — full per-slice logs live in this file's pre-2026-07-19 git history.
- **Fork-owned code to preserve in every merge**: `app/src/ai/agent_providers` (~18k-LOC BYOP), `byop_compaction/`, `byop_readiness/`, vendored `lib/rust-genai`, `crates/zap_sftp`, `crates/zap_sync`, the SSH remote-server flow, English-only UI, and the self-hosted updater. `app/src/ai/request_usage_model.rs` is the fork's always-unlimited local stub — NOT cloud code, never strip it.
- **Dep pins never taken from upstream**: `rmcp` = warpdotdev@`c0f65dc`; `reqwest 0.12.28` (+ reqwest-eventsource 0.6); `winit` = chenx-dust fork.
- **`warp_multi_agent_api` tracks upstream directly (2026-08-01)**: the mehmetbaykar/warp-proto-apis mirror and its `[patch]` override were dropped. The mirror only carried one zerx-lab commit (`14ab9a71`) deleting the `SearchCodebase` wire fields, and re-mirroring it on every upstream proto bump was a recurring merge tax (5+ bumps in the 2026-08-01 wave alone). Restoring those wire fields is additive — unused proto fields cost nothing — but code that *constructs* the restored messages must supply them: `crates/persistence/src/model.rs` sets `search_codebase_stats: None` because the fork has no codebase index. Bump the rev in `Cargo.toml` like any other dep from now on.
- **TUI removed (2026-07-19)**: `crates/warp_tui` is deleted — never shipped, no product value, and its half-merged waves broke gates twice. Upstream TUI commits are keep-deleted (strip-list). App-side shared seams STAY (they compile standalone and upstream churns them): `SettingsMode::Tui`, `app/src/tui/`, `app/src/tui_export.rs`, `.zap_cli` config-dir isolation in `warp_core::paths`, `command_is_warp_tui` pane detection, warpui_core's `tui` feature. A future terminal client would be resurrected from upstream's then-current code, not from history.
- **Merge gates** (all must be green): `cargo check --workspace` → 0; `./script/format`; and the test gate, which is **CI's exact command** — `cargo nextest run --workspace --locked --exclude command-signatures-v2 --profile ci --retries 2 -E 'not package(integration)'`. Baseline `script/upstream-merge-warp-baseline.txt` is **EMPTY**, so any FAIL is a real regression. Do **not** substitute `-p warp -p warp_core`: that covers ~5.1k of ~7.7k tests, and on 2026-08-02 it shipped a green local run while `crates/ai` had a genuinely broken `run_agents` result (orchestrate reported success when every agent failed to launch).
- **macOS gates cannot see non-macOS code — run `script/linux/local_gate`** before tagging. Two release rounds were burned (~17 min each) on breakage no macOS command could have caught: (a) `wgpu` is a *mandatory* dep under `[target.'cfg(not(target_os = "macos"))'.dependencies]` in `crates/warpui/Cargo.toml`, optional-only on macOS, so a stale version pin compiled fine locally; (b) `std::env::set_var` in a `cfg(not(windows/macos))` block needed an `unsafe` block under Rust 2024. Modes: `check` (workspace `--all-targets` — also type-checks every test target, so this *is* the Linux compile signal); `release` (the bundler's `--features release_bundle,crash_reporting`, **not** the default set — this mode alone caught two Rust-2024 breakages in the feature-gated, fork-owned `app/src/crash_reporting/local_minidump.rs`); `test` (CI's nextest; needs full codegen and OOM-kills rustc on a 15.6GB Docker VM — raise Docker memory or `ZAP_GATE_JOBS=1`, and note Linux test *execution* is also covered by tests.yml); `windows` (`cargo xwin check`, compile-only and scoped to `warpui*` — a full-workspace cross-check is blocked because openssl-sys must vendor OpenSSL for MSVC and its `Configure` rejects a Linux perl, leaving `app/`'s 37 windows-gated files to CI).
- **Releases**: push a `v2026.MM.DD.N` tag → three workflows build+publish (macOS signed app + musl CLI, Windows, Linux AppImage/deb/rpm). Latest: **v2026.07.19.2**. `tests.yml` runs the full 3-OS workspace nextest on pushes to main / PRs / dispatch. Tag-recut is safe while a release is unpublished: cancel runs, delete+recreate the tag.
- **Recent fork fixes** (details in git log): BYOP tool-call reliability wave `e94283114..8b13ec8d5` (Anthropic/OpenAI streamer hardening, real gzip-off + CRLF SSE tolerance, structured errors + 20s/300s timeouts, token-limit truncation reported as `truncated_output`; debug switch `ZAP_BYOP_DIAG=1` dumps full request JSON); SSH skill-load blank-pane fix `1fb6de95a` (remote bare-name skill paths + never-drop-task guard in `AddMessagesToTask`); stale remote file-tree fix `1901b3d2e` (`replace_children_of` prune + remote expand-refresh + failed-open self-heal; no wire change); `.zap_cli` TUI-config-dir isolation `49f3a1f78` (path fns remain in use).
- **Operational gotchas (hard-won — keep)**:
  1. `git rerere` replays bad resolutions — `git rerere forget <file>` before redoing a resolution.
  2. After any merge involving deferred/stripped subsystems, diff the WHOLE subsystem against pre-merge — clean auto-merges sneak in alongside conflicts (incl. `*_tests.rs` of kept-ours files). **Sweep method (2026-08-05 wave, which this caught 5 defects with):** for every file the merge touched, list the upstream commits touching it (`git log BASE..upstream -- <file>`); if they are ALL strip-commits, revert it to HEAD. That wave had 9 such files silently auto-merging onboarding paywall cards, Warp-hosted Factory MCP, TUI zero-state and session-sharing. Three traps: (a) sweep `--diff-filter=A` too, not just `M` — 5 more strip files arrived as *additions*; (b) `git rm`-ing DU paths leaves EMPTY DIRECTORIES, and `members = ["crates/*"]` then fails with "failed to load manifest for workspace member" — `find crates app -type d -empty -delete`; (c) an import can auto-merge in *outside* any conflict marker (a phantom `use ...::manager::Manager;` in `tab.rs`), so grep resolved files for symbols the fork deleted.
  3. Never run two cargo gate/build invocations concurrently (target/ corruption).
  4. History rewrites over merge commits need `-- --first-parent` (gpg-signed upstream commits rehash, ancestry severs).
  5. Gate-4 is **nextest** (process-per-test): `cargo test -p warp --lib` shows ~6 known thread-parallel flakes (`terminal::view::*`, `util::path::test_resolve_command`) that are not regressions.
  6. CI runs `--workspace`; any narrowed `-p` gate misses sibling-crate tests. This warning was already here on 2026-08-02 and was still under-followed — use CI's exact command, not a `-p` list.
  7. **Tag last.** `tests.yml` runs on push to `main`, the `v*` release workflows take ~2h. Push the commit, wait for 3-OS `Tests` to go green, *then* tag — otherwise a one-line compile error costs a full release cycle. Recut is only safe while the release is unpublished (cancel runs, delete+recreate the tag).
  8. Inside the Linux gate container, `warp` lib codegen OOM-kills rustc (`signal: 9, SIGKILL` with no "due to N previous errors" — looks like a phantom compile error). `local_gate` sets `CARGO_PROFILE_{DEV,TEST}_DEBUG=0` and `-j4` to stay under Docker's memory ceiling.
  9. `upstream-zap` (zerx-lab/zap, merged through `5d874456a`, tracked by merge) shares the fork's `v*` tag scheme — fetching it without `--no-tags` pollutes local tags.

### 5.1 Must-read conventions
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
- The formatter (`./script/format`) is configured with a `max_width` (max line length) of 100. Reflow comment line-wrapping to fill that full width rather than wrapping early at a narrower column, so comments span as few lines as possible.

**Comments**: comments have a cost — they carry a maintenance burden, because they must be kept in sync with the code they describe. It is tempting to assume more comments are always better, but be judicious about when one is actually necessary because the code cannot speak for itself.
- **Minimalist comments**: assume the reader is a senior engineer. Never comment to explain WHAT or HOW code works if self-documenting names accomplish that.
- **Strictly "why" only**: reserve inline comments for non-obvious rationale, workarounds for third-party bugs, complex algorithms, unidiomatic code, or unexpected edge cases.
- **No line-by-line narration**: never restate the syntax (omit `// Initialize array`, `// Loop over users`).

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
