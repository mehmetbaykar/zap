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

Current verification note (2026-07-09):
- `cargo check -p warp` and `cargo check -p warp --tests` pass (0 errors; 3 pre-existing unused-import warnings).
- `cargo test --no-run --workspace --exclude command-signatures-v2 --exclude managed_secrets_wasm` Finishes clean. `managed_secrets_wasm` remains the only build blocker: `ManagedSecretValue::openai_api_key(...)` is referenced in `crates/managed_secrets_wasm/src/lib.rs` but not defined in `crates/managed_secrets/src/secret_value.rs`.
- `cargo nextest run -p warp --no-fail-fast`: 3964 tests, 87 failed — the failing set is EXACTLY the pre-wave baseline red set at `91e64869d`/v2026.07.07.1 (verified by name-level diff against a baseline worktree run; zero regressions, every wave-added test passes). The 87 are the fork's known headless/fixture failures (e.g. the test app never registers the `AgentProviderSecrets` singleton for workspace/terminal view suites).
- `cargo nextest run -p warpui_core` 292/292 (the two long-failing transfer_view_tests are fixed); `-p warpui` 36/36 (incl. the new CGFont identity tests); `cargo check -p integration` clean.

Upstream sync state (2026-07-14 — MERGE ERA, see SPEC.md):
- **Slice 1 MERGED**: `git merge 8da83b42a` (upstream 2026-05-15 boundary; 503 commits from merge-base `c325d146a`) on branch `merge/upstream-catchup`, per SPEC.md's per-path conflict policy. 452 conflicted files resolved: 163 keep-deleted (cloud crates/modules, now tracked in `script/upstream-strip.list`), 46 keep-ours (autonomous zones per `.gitattributes` `merge=openwarp-ours` — the driver is now registered locally via `git config merge.openwarp-ours.driver true`; fresh clones run `script/setup-merge-drivers.sh`), ~10 manifests hand-merged, rest content-merged by a 24-batch Sonnet resolve+verify workflow plus three compile-fix waves (346 → 95 → 13 → 0 errors).
- Key slice-1 adaptations: kept the fork HTTP stack (reqwest 0.12.28 + warpdotdev/rmcp git pin + reqwest-eventsource 0.6) instead of upstream's reqwest 0.13/rmcp 1.6; adopted upstream's `crates/mcp` extraction (compiles against the fork rmcp pin); stripped proto-only conversions that need a newer `warp_multi_agent_api` (RunAgents cluster, orchestration_config/status protos, custom_model_providers, ShellCommandFinished start/finish_ts); restored the fork's `request_usage_model` always-unlimited stub (it is NOT cloud code — 11 consumers); kept local orchestration UI (OrchestrationConfig native struct, pin model + pill bar, `LocalClaudeCodexChildHarnesses`); local Codex child panes validate the CLI directly (standalone codex.rs driver deferred); `hide_continue_actions` tombstone slice ported. Deferred to later slices/workstreams: upstream agent_sdk cloud-era files, harness_availability (needs shim), handoff, codebase embedding index.
- Slice-1 gates (all green, 2026-07-11): `cargo check --workspace` 0 errors; `cargo test --workspace --no-run` finishes with 0 errors (the old `managed_secrets_wasm`/`openai_api_key` exception is FIXED — `ManagedSecretType::OpenaiApiKey` restored); `-p warpui_core`+`-p warpui` 328/328; `cargo nextest run -p warp`: **46 failed of 4233, all a subset of the 87-name pre-slice baseline — zero new failures, 41 baseline reds healed** (mostly by registering the missing test singletons: `OrchestrationPinModel`, `AgentProviderSecrets`, `CLIAgentInstallModel`, `ProxyCredentials`, `CloudSyncTokenStore`, ssh-manager temp DB path, and `Language/Network/Autoupdate` settings groups in the shared test init). New `-p warp` baseline: 46 names.
- warp_tui does not exist at this boundary (born upstream ~June); the SPEC's `cargo check -p warp_tui` gate applies from the slice that introduces it.
- **Slice 2 MERGED**: `git merge a44b70306` (upstream 2026-06-01 boundary; 313 commits after slice 1). The merge produced 959 conflicts, dominated by the 2026-05-30 import-format rewrite. Resolution kept local BYOP/CLI/agent orchestration and adopted upstream product work including queued prompts, async find, tab groups, local harness setup, Computer Use executors, remote codebase-index status, custom endpoint token usage, and GitHub PR prompt-chip runtime policy.
- Slice-2 cloud strip: rejected Warp API-key management, Warp-server IAP, local-to-cloud handoff/snapshots, cloud/RTC task sync, cloud auth-secret deletion, session-sharing QR/web-view additions, and orchestration credit/billing rollups. Recurring paths are recorded in `script/upstream-strip.list`; `app/src/ai/request_usage_model.rs` remains the fork's always-unlimited local stub. Proto-only fields requiring a newer `warp_multi_agent_api` remain omitted.
- Slice-2 gates (all green, 2026-07-12): `cargo check --workspace` clean; `cargo test --workspace --no-run` clean; `-p warpui_core` + `-p warpui` **328/328**; `cargo nextest run -p warp --no-fail-fast` **4421 passed / 44 failed / 9 skipped of 4465**, with **zero new failing names** versus the 46-name slice-1 baseline. Two baseline failures healed (`ai_document_model::test_plan_markdown_content_preserves_copyable_structure`, `searcher::test_tokenizer_warp_special_chars`). The rolled 44-name baseline is tracked at `script/upstream-merge-warp-baseline.txt`.
- **Slice 3 MERGED**: `git merge 09be9c1ff` (upstream 2026-06-15 boundary; 229 commits after slice 2). Resolution adopted upstream product work including local-control/`warpctrl`, local GitHub repository polling for code review, LSP/codebase-status improvements, local app bootstrap, and local child-agent orchestration actions (`StartAgent`, `RunAgents`, `WaitForEvents`). The fork's BYOP routing, CLI harnesses, stable parent/child run IDs, pane restoration, and older `warp_multi_agent_api` pin remain intact. Direct Grok OAuth is retained only as a provider-to-app primitive; it never traverses Warp infrastructure.
- Slice-3 cloud strip: rejected Agent SDK cloud providers/API keys/artifact upload/observability, Warp server API/IAP/admin/billing surfaces, shared-session network and cloud-conversation replay, task sync/event streaming, remote-server auth, tracing/telemetry, and cloud-only persistence. Legacy `warp://conversation/*` and `warp://drive/*` routes are inert compatibility sinks with no token/ID parsing or dispatch. Recurring paths are recorded in `script/upstream-strip.list`; `app/src/ai/request_usage_model.rs` remains the always-unlimited local stub.
- Slice-3 gates (all green, 2026-07-13): `cargo check --workspace` clean; `cargo test --workspace --no-run` clean; `cargo test -p warpui_core -p warpui` **335 passed / 0 failed / 9 ignored** across unit and doc tests; `cargo nextest run -p warp --no-fail-fast` **4659 passed / 44 failed / 9 skipped of 4703 run**, with the failing-name set exactly equal to the 44-name slice-2 baseline (**zero new failures**). The existing baseline file therefore remains unchanged. The full nextest run also exercises the deterministic BYOP provider, MCP, skills, file-edit, and SSH-manager smoke coverage; live provider chat remains credential-dependent.
- Slice-3 explicit post-catch-up work: add the local inter-agent mailbox/`SendMessageToAgent` CLI contract; wire Grok inference/secret-storage UI around the retained OAuth primitive; port the local embedding-backed `SearchCodebase`, `FetchConversation`, `/init`, and active-agent dashboard. These are local product features, not permanent strip entries.
- **Slice 4 MERGED**: `git merge 474cf6b0` (72 commits after slice 3). Resolution adopted upstream product work including queued-prompt/LRC policy and localized queue hints, local `RunAgents` cards plus Stop/Kill child-agent actions, remote Git/GitHub code-review support, remote/global-search improvements, a bounded local network-log pane, onboarding updates, and WarpUI structural descendant tracking. The fork's BYOP request routing, local CLI orchestration, pane persistence, SSH manager, and older `warp_multi_agent_api` pin remain intact.
- Slice-4 cloud strip: rejected Agent SDK hosted workers/environments and observability, Warp server API/task sync/auto-handoff, shared-session network additions, managed cloud credential flows, billing/credits, and telemetry/Sentry. New LRC settings are explicitly local-only; the network log is bounded in memory and has no upload path. All 214 recurring strip-list paths are absent, and `app/src/ai/request_usage_model.rs` remains the always-unlimited local stub.
- Slice-4 gates (all green, 2026-07-13): `cargo check --workspace` clean; `cargo test --workspace --no-run` clean; `cargo test -p warpui_core -p warpui` **335 passed / 0 failed / 9 ignored**; `cargo nextest run -p warp --no-fail-fast` **4714 passed / 43 failed / 9 skipped of 4757 run**, with **zero new failing names** versus the 44-name slice-3 baseline. `terminal::view::tests::ctrl_c_after_stop_takeover_cancels_conversation` healed, so `script/upstream-merge-warp-baseline.txt` rolled to 43 names.
- **Slice 5 MERGED**: `git merge 1c376cb0f` (273 commits after slice 4). Resolution adopted upstream's non-default `crates/warp_tui` plus the ratatui-backed WarpUI runtime/elements, local TTY Agent surface, TUI skills/context discovery, input/file-edit/tool-call support, terminal-surface abstraction, active-agent views, local diff storage/input-mode policy, Custom Routers, and Antigravity CLI support. The fork's direct BYOP routing, local Claude/Codex/OpenCode child harnesses, pane persistence, SSH manager, and older `warp_multi_agent_api` pin remain intact.
- Slice-5 cloud strip: rejected TUI telemetry/autoupdate, hosted `run-cloud`/auth-secret behavior, Warp-account onboarding, subscription/billing/team policy, cloud handoff/sync/memory, recording upload, and Warp-server APIs. `report_error!` is a local-log-only compatibility shim; Grok OAuth refresh is provider-direct to xAI with no Warp account/team gate; `RemoteAgentContext` uses only the local SSH remote-server transport. Every recurring strip-list path remains absent, and `app/src/ai/request_usage_model.rs` remains the always-unlimited local stub.
- Slice-5 gates (all green, 2026-07-14): `cargo check --workspace` clean; `cargo test --workspace --no-run` clean; explicit `cargo check -p warp_tui` clean; `cargo test -p warpui_core -p warpui -p warp_tui` **574 passed / 0 failed / 9 ignored**; `cargo nextest run -p warp --no-fail-fast` **4839 passed / 43 failed / 9 skipped of 4882 run**, with the failing-name set exactly equal to the 43-name slice-4 baseline (**zero new failures**). The baseline file remains unchanged.
- **Slice 6 MERGED**: `git merge 5e9dc1c24` (23 commits after slice 5, reaching the pinned upstream tip). Resolution adopted upstream's TUI running-block refresh, independent GUI/TUI persistence scopes, hidden-section double-click expansion, local Computer Use recording cards, shared slash-command mixer, local Custom Router feature intro, orchestration pane-drag fix, MCP search sizing, terminal theme probe, macOS Computer Use keycode fix, TUI zero state/editor/inline diffs, and background Computer Use on X11. Direct `warp_errors` imports terminate in the fork's local-log-only implementation. BYOP routing, local child-agent harnesses, TUI, pane persistence, SSH manager, and the older `warp_multi_agent_api` pin remain intact.
- Slice-6 cloud strip: rejected Oz web run links, TUI credit/cost accounting, out-of-credits subscription CTA, recording upload/server summaries, hosted workers, Warp auth/server APIs, cloud sync, shared-session network/token additions, and telemetry/Sentry. Every recurring strip-list path remains absent, `app/src/ai/request_usage_model.rs` remains the always-unlimited local stub, and newer-proto-only usage/recording fields remain omitted.
- Slice-6 gates (all green, 2026-07-14): `cargo check --workspace` clean; `cargo test --workspace --no-run` clean; explicit `cargo check -p warp_tui` clean; `cargo test -p warpui_core -p warpui -p warp_tui` **593 passed / 0 failed / 9 ignored**; `cargo nextest run -p warp --profile ci --no-fail-fast` **4847 passed / 43 failed / 9 skipped of 4890 run**, with the failing-name file byte-for-byte equal to the 43-name slice-5 baseline (**zero new failures**). The baseline file remains unchanged.
- Catch-up complete through `5e9dc1c24`; resume steady-state upstream merges by listing commits after that target and applying the same de-clouding policy and gates.
- **Slice-7 wave MERGED (2026-07-16)**: 98 upstream commits `5e9dc1c24..19ebec9da` in 33 micro-merges (`zap-merge-slice-7.1`..`7.33`, batch alias `zap-merge-slice-7`), full SPEC gate green on every slice. Adopted: TUI slash commands/inline menus/conversation menu/model selector/skills browser/word wrap/Markdown/alt-screen rendering with PTY input forwarding, TUI conversation persistence + local-DB restore, shared GUI/TUI slash+picker helpers (`TuiSlashCommand`, `query_model_picker_choices`, `argument_hint`), live-cwd repo-gated slash commands (#13614), LRC support + `UpdatedInstruction` subagent events + dispatch-success bool, keychain service isolation `.tui` suffix (#13692), Info.plist EventKit usage strings, box-drawing glyphs flag, async-find graduated to default, RunAgentsTool flag removed, `TerminalLifecycleRecovery` default-ON (#13788 — fork's guarded InitShell block-start KEPT: it restores upstream's started-bootstrap-block invariant that the fork's no-start-at-creation divergence breaks; 71 lifecycle tests green). Fixed upstream restore idempotency bug (plan re-inserted blocks the GUI snapshot already held; skip-existing-ids guard added).
- Slice-7 strips (one line each): #4 billing dropdown; #9/#48/#59/#69/#72 telemetry re-removals; #12 TUI credits display (duration kept); #25 cloud-env deser; #26 --api-key TUI login; #30 cloud-mode queued-prompt Copy; #38 recording finalize/upload family (proto-pinned); #44 leftovers per #47; #60 device-sync coupling (recognized-model guard kept); #63/#83 ci.yml; #76 already-picked dedupe; #85 GEAP credentials + recovery widget (icon enum+SVG inert); #86 orchestration module (imports stripped cloud stack, no consumer yet — comes with its TUI card); #88 TUI login-resume (device-auth); #96 oz_handoff/handoff_local_cloud flags; CloudRunners flag; shared_session_long_running_commands default; server-token conversation restore (local-DB only); TUI exit-summary/resume plumbing; usage toggle.
- Slice-7 final gate (2026-07-16): `cargo check --workspace` clean; `cargo test --workspace --no-run` clean; `cargo check -p warp_tui` clean; warpui suites **341/341**; `cargo nextest run -p warp` **4949 passed / 0 failed / 9 skipped** — baseline stays EMPTY. Security sweep over all 341 wave-touched .rs files: zero new outbound endpoints, telemetry macros remain `if false` no-ops. Next wave resumes after `19ebec9da`.
- **Weekly wave MERGED (2026-07-18)**: `git merge 8c55ad2b8` — 53 upstream commits `19ebec9da..8c55ad2b8` — on branch `merge/upstream-weekly-2026-07-17` (merge commit `92880fa38`; main `f916f320e` untouched, user reviews before main). Adopted into default members: security bumps (serde_with 2.3.3→3.21.0 for GHSA-7gcf-g7xr-8hxj #13882, ws 8.18.3→8.21.0 #13564); crash/reliability (#13789 `TaskStore::exchange_by_id` not-yet-linked subtask crash, #13802 Windows ConPTY root-process terminate + pseudoconsole close on kill, #13617 flaky `test_handle_pty_read_event_while_not_batching`); terminal/UX (#12896 hide inactive slash-command hints, #13801/#13433 exclude directories from the `files:` command-palette filter, #13823 pane-header cloud-icon fix for shared **local** conversations, #13750 MCP tool-chip padding, #13767 pinned tabs → stable, #13224 tab_config window max-height + scroll, #13803 PowerShell history embed-path read, #13816 global `--api-key`/`--debug`-before-subcommand parse fix, #13784 claude-code `StopFailure` hook, #13744 V0 MCP app-side); agent (#13059 hide orchestrated child-agent conversations from the list, #13773 `agent_identity_uid` for local remote-server child execution); computer_use recording **infra restored dormant/unwired** (prefer-theirs shared core: #13742 macOS avfoundation, #13645 Linux x11grab, #13795 `-draw_mouse` cursor visibility + 4× playback, #13447 keyboard-action overlay burn-in); Windows bootstrap deps #13532; **telemetry removal** #13847 (13 unused events — aligns no-analytics); mitigation #13790 `nld_prompt_history_match` off (dropped from DOGFOOD_FLAGS).
- Weekly-wave strips (cloud/subscription/IAP/telemetry): #13783 VA in-progress-recording **upload** on VM early exit; #13624 + Oz-runner IAP tokens via WIF/injected-OIDC-JWT; #7ffccbb82/#13760 `oz runner` CRUD CLI (cloud runners); #13794 TUI sentry/telemetry-flush init; `ai::agent::document_action_presentation` (VA/cloud action variants — module stripped, `pub mod` removed from agent/mod.rs); `Kiro` skill provider (fork `warp_multi_agent_api` proto lacks the variant — enum arm, icon arm, definition block, 2 conversion arms removed). `app/src/ai/request_usage_model.rs` remains the always-unlimited local stub; no new outbound endpoints.
- Weekly-wave DEFERRALS (documented, not lost): (1) **TUI wave** — the merge pulled ~20 TUI commits (#13775 multi-session registry, #13830 AskUserQuestion, #13717 orchestration permission card, #13716 option selector, #13817 editor view, #13781 plan toggle, #13731 inline plans, #13826 NLD, #13730 semantic Markdown, #13797 conversation-list left-arrow, #13696 alt-screen mouse, #13738 hide outer agent bar, #13850 footer padding, #13805 chevron marker, #13838 `×` failed-tool glyph, #13771 editor input routing, #13714 GUI orchestration pickers on option snapshots, #13729 editor-backed code blocks, #13833 TUI clippy, #13782 Kitty keyboard protocol) as a **half-merged 28-file/+3479-line increment**: conflict resolution declared 9 new `crates/warp_tui/src` modules (`orchestration_block`, `tui_ask_question_view`, `tui_code_block_view`, `tui_plan_view`, `editor_view`, `editor_interaction`, `option_selector`, `mcp_menu`, `orchestrated_agent_identity_styling`) but **never created the files**, and warp_tui took theirs referencing app-side `tui_export` exports (`TuiMcpAction`, `should_intercept_mouse`/`should_intercept_scroll`) absent from the fork's reverted `tui_export.rs` plus the stripped `document_action_presentation`. Per SPEC "TUI → strip if it blocks the gate (non-default member)": **reverted `crates/warp_tui` entirely to last-green main** (66 errors → 0). Resume as a dedicated TUI slice that creates the 9 modules, adds the `tui_export` exports, and de-clouds `document_action_presentation`. (2) **#45 / #12328 repo-metadata async tree-walk cancel-on-teardown** — reverted the whole footprint to main (`crates/repo_metadata/*`, `crates/ai/src/index/file_outline/native.rs`, `app/src/code/file_tree/*`, `app/src/remote_server/server_model.rs`, incl. the `load_directory_with_completion` caller): the async `build_tree()`/`load_directory_with_completion` API diverges from the fork's sync repo_metadata and dragged in cloud-diverged file_tree APIs (`server_api`, `LocalOrRemotePath`, `OpenFile.location` vs fork `.path`); partial adoption reddened 9 `code::file_tree::view` tests, full revert greens them. (3) **OSC 8 hyperlinks app-side #9850** — integration test registrations stripped, orphan comment removed, app-side deferred.
- Weekly-wave fork-fix (separate commit `80d1531c6`, not folded into the merge): `test_submit_queued_prompt_detects_slash_command` was an **inherited-from-upstream ~25% flake** (verbatim: identical test, `submit_queued_prompt`, and missing arm at tip `8c55ad2b8`). `active_commands_by_id` is a `HashMap`, so the data-driven test nondeterministically selects `/orchestrate`; orchestrate is classified submitted-as-prompt yet had no `execute_slash_command` arm, so it hit the `_ =>` no-handler `debug_assert!`. Added `ORCHESTRATE` to the INIT/PLAN "just sends AI request with prefix" `return false` arm — deterministically green 15/15 isolated.
- Weekly-wave final gate (2026-07-18): `cargo check --workspace` clean; `cargo check -p warp_tui` clean; `cargo nextest run -p warp` **4898 passed / 0 failed / 9 skipped** — baseline stays EMPTY (the earlier `/orchestrate` flake and a one-off `notebooks::editor::model::tests::test_edit_command_submodel` SIGSEGV both verified non-reproducible after the fix: SIGSEGV 8/8 clean isolated, slash 15/15). Note: `cargo test -p warp --lib` (thread-parallel) still surfaces ~6 shared-process-global flakes (`terminal::view::*`, `util::path::test_resolve_command`) that all pass single-threaded and under nextest's process isolation — gate-4 is nextest, per prior slices. Next wave resumes after `8c55ad2b8`.
- **Weekly wave-2 MERGED (2026-07-18, same branch)**: `git merge 0017f3059` — 17 upstream commits `8c55ad2b8..0017f3059` (all 2026-07-17). Adopted (9): OSC 7 Windows drive-path cwd fix #13920; **editor auto-save** #13435 (see below); idempotent render-test logging #13889; wasm-bundle feature-env mapping #13930 (inert — fork never passes those features); 5× `report_error!`→`log::warn!` demotions #13905/#13910/#13922/#13923/#13924 (kept-ours on 5 hunks whose call sites live in fork-stripped cloud code: input.rs ambient-agent cloud-handoff + `server_api.predict_am_queries` (fork uses BYOP `nld_predict`) + session-sharer `submit_viewer_ai_query`; view.rs codebase-index `init_project`/`generate_codebase_index`; ansi demotions taken with Zap-brand strings).
- Wave-2 deferrals (8, all TUI backlog): the CODE-1822 orchestration stack #13831→#13776→#13777→#13832 (tab-bar component, local child agents, rich message, orchestration tab bar — new modules `orchestration_model`/`agent_message`/`tab_bar` + warpui_core tui primitives swept AND the auto-merged warpui_core flex/text edits reverted; app-side spillover verified NOT separable: `child_agent_launch.rs` overlaps fork's `local_harness_launch.rs`, `tui_export` needs absent `orchestration_event_streamer`, `llms.rs` `set_agent_mode_llm_override` reorder rejected — `llms_tests.rs` also reverted, upstream's tests reference cloud `AuthManager`/`CloudModel`/`NetworkStatus`), NLD-in-slash #13893 (fork lacks the `terminal_session_view/` dir-refactor + NLD subsystem), /exit+ctrl-d #13915 (imports missing `editor_interaction`), **#13901 bootstrap-stage input gating** (initially takeable but its `terminal_use.rs` is entangled with auto-merged consumers of deferred modules — whole `crates/warp_tui` reverted to pre-merge per the SPEC strip rule after the half-and-half state broke `tui_column_layout`/`root_view`/`tui_file_edits_view`/`tui_shell_command_view` (`ToolCallDisplayState::{glyph,glyph_style,label_style}` providers kept-ours)), and **#13904 "rename warp-tui to warp" — cosmetic only** (clap display name + strings, zero Cargo/bin changes; at un-defer time keep `warp-tui`/`zap-tui` and NEVER add bare `"warp"` to `command_is_warp_tui` — it would misclassify the fork's `warp` GUI bin). LESSON: a merge with a deferred-module stack must sweep not only conflicted files but ALSO clean auto-merges inside the deferred subsystem (warp_tui, warpui_core tui, *_tests of kept-ours files) — diff the whole subsystem against pre-merge, not just the conflict list.
- Wave-2 auto-save adoption detail: the fork had its OWN earlier autosave (`code.editor.autosave` `AutosaveMode` in the external-editor `EditorSettings` group, default AfterDelay) — upstream #13435 (`CodeSettings.auto_save`, `code.editor.auto_save`) is a superset with a critical guard the fork's lacked (`diff_type.is_none()` — never auto-save pending agent edit-file proposal buffers), plus focus-change save, auto-save toast suppression, untitled/remote-disconnect guards, quit-warning/code-review flush integration. Resolution: ported upstream's implementation ONLY (3-way apply of the commit's `local_code_editor.rs`/`code/view.rs` diff onto fork files — a naive whole-file `--theirs` take was reverted because it dragged in upstream-only `lsp_telemetry`/`buffer_location::LocalOrRemotePath` refactors; **gotcha: `rerere` re-applied the bad whole-file resolutions on the retry — `git rerere forget <file>` before redoing a resolution**), removed the fork's now-dead autosave dropdown from `settings_view/features/external_editor.rs`, kept the fork setting field for config compat (commented superseded), defaulted upstream's `auto_save` to **true** (fork shipped autosave-on; upstream defaults false), kept fork's unit-variant `CodeReviewTelemetryEvent::FileSaved` (no `is_local`/`repo_is_local` — fork telemetry is a local no-op) plus fork's `CodeReviewViewEvent::FileSaved` emit (mermaid auto-refresh consumer), and grafted `AutoSaveToggleWidget` into the fork's restructured `build_page` (both cfg variants) while rejecting upstream's `CodeSubpage` settings refactor + LSP-suggestion actions (fork settings page diverged).
- Wave-2 final gate (2026-07-18): `cargo check --workspace` clean; `cargo check -p warp_tui` clean; `cargo test --workspace --no-run` clean; `cargo nextest run -p warp` **4904 passed / 0 failed / 9 skipped** (+6 new auto-save tests) — baseline stays EMPTY. Next wave resumes after `0017f3059`.
- **BYOP tool-call reliability wave (2026-07-18, commits `e94283114..8b13ec8d5`, same branch)**: 4-agent audit (architecture map, adversarial stream bug-hunt, upstream-drift diff, provider-spec conformance) of intermittent native-agent tool-call failures (mid-request errors + malformed/truncated args through an SSE-normalizing LiteLLM-style proxy; provider verified fine directly). Upstream-alignment verdict: shared agentic core (crates/ai, mcp, app agent) matches upstream tip `0017f3059` — zero missed upstream fixes; the whole provider-stream layer is fork-owned (vendored `lib/rust-genai` + `agent_providers/chat_stream.rs`), so all defects were fork-internal. Six fixes, cross-reviewed (sonnet adversarial pass, 2 findings folded back in): (1) Anthropic streamer — surface mid-stream `event: error` frames (were silently swallowed → truncated/empty turns), resilient `content_block_stop` arg finalize (raw-string fallback instead of `?`-abort on corrupted `input_json_delta`), dispatch falls back to the JSON `type` field when a proxy strips SSE `event:` names; (2) OpenAI streamer — capture ALL `delta.tool_calls[]` not just `.first()` (batching relays lost parallel calls), index-gap resize uses empty placeholders (was cloning the current call into gap slots → cross-contaminated args), empty-args `""`→`{}` normalize, assistant echo-back no longer double-encodes `Value::String` arguments; (3) transport — `.gzip(false)` actually applied (the documented SSE-anti-buffering default was ineffective: reqwest's `gzip` feature auto-negotiates unless explicitly disabled), CRLF `\r\n\r\n` SSE separators normalized before the `\n\n` split (proxies emitted them → whole response buffered to EOF then parse-failed); (4) app `chat_stream.rs` — BYOP errors mapped to structured `AIApiError::ErrorStatus`/`Stream` instead of always-retryable `Other` (4xx no longer waste the one-shot resume), connect 20s + read-idle 300s timeouts (streams previously had NO deadline), `stop_reason=max_tokens`/zero-`End`-event truncation detected and reported to the model as `truncated_output` (retry smaller) instead of `invalid_arguments` (fix-the-schema loop); (5) `crates/ai` — empty CreateDocuments title defaults to in-crate `DEFAULT_PLANNING_DOCUMENT_TITLE` (removed a `DO NOT SUBMIT` placeholder). genai lib tests 70/70; full gates green after the wave (nextest 4904/4904). Debug switch for future incidents: `ZAP_BYOP_DIAG=1` dumps full request JSON + error-position byte context.
- **SSH skill-load blank-conversation fix (2026-07-19)**: loading a skill in a warpified SSH session blanked the whole agent pane (endless "Warping…"). Compound bug: (1) BYOP's `read_skill` tool packs the bare skill NAME into the proto `SkillPath` slot; with `SkillPathOrigin::Remote` the decode required `StandardizedPath::try_new` (absolute-only) → `RemotePathInvalid` → `MissingSkillReference`, deterministic for every skill on SSH; (2) `Action::AddMessagesToTask` (conversation.rs) `remove`s the task from the store and `?`-bailed on the conversion error BEFORE the reinsert — the entire root task (the whole conversation) was dropped; every mounted block re-queried `exchange_with_id` → `None` → `Pending` → blank transcript, while the footer spinner keys off conversation status and spun forever. Three-layer fix: conversion fallback (Remote + non-absolute path → display-compatible Local identity, same rationale as `RestoredDisplayOnly`; executor's `find_skill_by_name` resolves by name anyway), `ReadSkill` arm in `convert_from.rs` degrades conversion errors to `NoClientRepresentation` (mirrors `SuggestPrompt`), and `AddMessagesToTask` ALWAYS reinserts the task before propagating errors (blast-radius guard for the whole error class). Regression tests in `crates/ai/src/skills/conversion_tests.rs` (bare-name fallback + absolute-path-stays-remote).
- **First full-workspace CI run (tests.yml, 3-OS matrix, 2026-07-18)** on the branch caught one real fork bug the local `-p warp` gate can't see: `warp_core::paths::tests::test_tui_mcp_config_path_is_separate_from_gui` (added by adopted #13744) failed deterministically on macOS — `macos_tui_config_dir_name` rewrites `.warp`→`.warp_cli`, but the fork's Oss channel renames the GUI dir to `.zap`, so the rewrite matched nothing and the TUI config dir collided with the GUI's (shared `.mcp.json`). Fixed in `49f3a1f78`: Oss → `.zap_cli` + a `{gui}_cli` fallback for any future rename; warp_core 88/88. Windows job green (leaky-but-passing tests only). LESSON: CI runs `--workspace` — local gate-4 (`-p warp`) misses sibling-crate tests; check `-p warp_core` (and friends) when paths/channel code changes.
- **Test-debt burndown (2026-07-14, post-slice-6):** all 43 inherited `-p warp` baseline failures fixed — `cargo nextest run -p warp --no-fail-fast` is now **4890 passed / 0 failed / 9 skipped** and `script/upstream-merge-warp-baseline.txt` is empty (the gate-4 name-diff must stay empty from here on). Root causes and fixes: (1) test fixtures never called the fork's fluent `i18n::init` — registered `init(Some("en"))` in `initialize_settings_for_tests_with_mode` plus two standalone fixtures, healing every raw-fluent-key assertion (notebooks, drive export, vertical tabs, prompt menus); (2) fluent wrapped `t!()` arguments in Unicode bidi-isolation marks (U+2068/U+2069) that leaked into rendered UI strings — `set_use_isolating(false)` now re-applied after every bundle (re)load in `app/src/i18n.rs` (it only affects already-loaded bundles); (3) the de-cloud had stubbed `UserWorkspaces::{current_team, current_team_uid, current_team_mut, team_from_uid, team_from_uid_across_all_workspaces}` to `None` and `owner_to_space`'s `Owner::Team` arm to `Shared` — restored upstream bodies (production-neutral: no workspace/team is ever populated without cloud fetch), healing the 8 `ai::blocklist::permissions` workspace-override tests and `cloud_object` space classification; (4) **real product bug**: `Dropdown::select_action_and_close` dispatched the `Box<dyn DropdownItemAction>` itself, so handler lookup keyed on the box's `TypeId` and never matched any view — every dropdown item action in the fork was a silent no-op; added `dispatch_boxed_typed_action_deferred` to warpui_core and upcast the box, restoring dropdown→view action delivery (SSH OneKey dropdown test now passes); (5) **real product bug**: `should_confirm_close_session` was hardcoded `false` and the `CloseSharedSessionPaneRequested` handler skipped the dialog — restored the surviving local gate (`SessionSettings::should_confirm_close_session`, cloud feature-flags dropped); (6) session-sharing tests: the sharer/viewer network handshake is stripped, so fixtures now set the terminal's shared state directly (`on_session_share_started` / `on_session_share_joined` + explicit `SharedSessionStatus`), `stop_sharing_session_for_reason` performs the local teardown the network layer used to do, and `close_tabs` stops shares in closing tabs so restored tabs come back unshared; (7) zerx's mass Warp→Zap rename had corrupted grid-regex/URL test *data* (expectations unshifted) — restored upstream fixtures verbatim; `open_in_warp` test now points at the renamed `app/src/bin/zap_oss.rs`; (8) fork-adapted expectations for de-clouded UI: drive index menu (Share removed, Trash present), unified new-session menu (AI always enabled ⇒ Agent group precedes config group), shared-session web URLs no longer rewrite to intents; (9) missing test singletons registered (`AvailableShells`, `RemoteServerManager`, `KeybindingChangedNotifier`, ssh-manager sqlite temp path). Also fixed the flaky `warp_tui` `diff_pipeline_computes_added_lines_and_ghost_blocks` (bounded 100-iteration yield poll starved under load; now 100k) and translated all remaining Chinese code comments to English (~120 lines across 26 files; CJK **string-literal test data intentionally kept** — wide-char/tokenizer coverage). UI/TUI combined suite: 593 passed / 0 failed / 9 ignored.

Upstream sync state (2026-07-09, cherry-pick era — historical):
- Latest triaged upstream/warp (warpdotdev/warp) tip: `4c4ab7506` (2026-07-09). Of the 3 commits after `ed34ab5e5`, two are TUI-only (excluded) and #13441 removes a telemetry event this fork never had — nothing to port. A full re-audit of the 134 commits `178fe89bc..ed34ab5e5` (multi-agent scan + adversarial verify, 2026-07-09) found five product fixes the 2026-07-08 triage wrongly skipped; all five are now applied: launch-config tab commands via warp://launch (#13103), Markdown-Viewer preference for file:// URLs (#12866), macOS CGFont-identity glyph misrender fix (#13317), DragTabsToWindows promoted to RELEASE_FLAGS (#13411), and FxHash `EntityIdMap`/`EntityIdSet` in warpui_core hot paths (#13058, manual port — the fork's presenter keeps its own layout-time parents map, window views stay `Box<dyn AnyView>`, TUI hunks dropped). #13157 (FTU callout removal) re-confirmed as correctly skipped. The Precmd-lifecycle wave's copied tests were also adapted to Zap APIs (no PreexecValue.session_id / hook-session validation, stubbed telemetry) so `cargo check -p warp --tests` compiles again. Resume future warp syncs by listing commits after `4c4ab7506`.
- 2026-07-09 (later same day): triaged the 6 commits `4c4ab7506..883f22b00`; ALL skipped with verified rationale — #13494 free-tier modal removal (surface already absent here), #13497 TUI skills/context discovery (TUI excluded), #13499 Custom Routers crash fix (custom_router settings view not ported), #13476 team BYO policies (team/subscription), #13502 run-cloud CLI flags (cloud agents), #13483 log::error!→report_error! 345-file migration (Sentry-reporting infra; this fork physically removed telemetry, so the migration has no product value and a huge merge surface). Also triaged `1c376cb0f` (#13460, block agent requests on grok oauth token refresh): SKIP — patches the grok_subscription / server_api cloud-OAuth subsystem this fork doesn't carry. Resume future warp syncs by listing commits after `1c376cb0f`.
- 2026-07-08 triage (for reference): last clean cherry-pick baseline `178fe89bc` (#13097); reviewed all 134 commits through `ed34ab5e5`. Applied product fixes where they merged cleanly (or via small local ports), including a second wave of previously-deferred items: settings deeplink (#13232, drop `custom_router`), NLD↔ai_queries history (#12586), LRC/CliAgentUserQuery race (manual onto PendingUserQuery, #13191), Precmd lifecycle stack (#12853→#12859), and Jupyter `.ipynb` client wiring (#13071, local FS). Skipped cloud/subscription/billing, TUI-only, tab-groups, orchestration/model-router, VA recording, CI/skills-lock, memory/Claude cloud task sync, `/repos` lazy git, and `#13202` queued inline editor.
- upstream/zap (zerx-lab/zap) is merged through `5d874456a` (2026-07-09: BYOP provider routing #306 + tool-execution #305, settings context-menu Cut/Copy/Paste #307, Linux Ctrl+V paste binding #304; their newly-tracked Cargo.lock resolved keep-ours). zap is tracked by **merge**, warp by selective **cherry-pick**. Their new BYOP smoke tests needed fork-side fixture singletons (AIExecutionProfilesModel/ObjectStoreModel/TemplatableMCPServerManager) because this fork's LLMPreferences runs upstream-warp #10085's disabled-model reconciliation, which zerx's tree lacks.

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
