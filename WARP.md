# WARP.md

This is the engineer handbook for Zap. `AGENTS.md` is the crate and module map; read this file first for commands, process, and repository-wide rules.

## Development commands

### Build and run

- `cargo run --bin warp` builds and runs the local, accountless Zap application.
- `cargo bundle --bin warp` builds the native application bundle.
- `cargo check --workspace` is the required compile gate for ordinary changes.

Zap intentionally has no Warp-server development mode. Do not restore `with_local_server`, Warp GraphQL, Warp auth, billing, subscriptions, telemetry, Sentry, or cloud-sync clients.

### Testing

- `cargo test --workspace --no-run` builds every workspace test target.
- `cargo nextest run --no-fail-fast --workspace --exclude command-signatures-v2` runs the workspace test suite.
- `cargo test -p warpui_core -p warpui` runs the WarpUI gate.
- `cargo nextest run -p warp_completer --features v2` runs the v2 completer tests.
- `cargo test --doc` runs documentation tests.

During upstream merges, follow `SPEC.md`: every wave must pass the workspace check, full test-build, WarpUI tests, and the `warp` failing-name baseline diff. (`crates/warp_tui` was removed 2026-07-19 — upstream TUI commits are keep-deleted per `script/upstream-strip.list`; no TUI gate exists anymore.)

### Formatting and linting

- `./script/presubmit` runs the repository presubmit checks.
- `./script/format` formats supported sources.
- `cargo clippy --workspace --all-targets --all-features --tests -- -D warnings` runs Clippy.
- `./script/run-clang-format.py -r --extensions 'c,h,cpp,m' ./crates/warpui/src/ ./app/src/` formats native sources.

Do not reformat unrelated files while resolving an upstream merge. Every changed line must belong to the requested slice or its compile/test repair.

### Platform setup

- `./script/bootstrap` performs platform setup and installs the common skills pinned in `skills-lock.json`.
- `./script/bootstrap --skip-common-skills` performs platform setup without changing common skills.
- `./script/install_cargo_build_deps` installs build dependencies.
- `./script/install_cargo_test_deps` installs test dependencies.

## Architecture

Zap is a Rust terminal and local agentic development environment built on WarpUI.

- `app/` assembles terminal, editor, Agent, settings, persistence, and platform integrations.
- `crates/warpui` and `crates/warpui_core` provide the declarative UI framework and entity-handle runtime.
- `crates/ai` contains provider-independent Agent protocol and tool contracts.
- `app/src/ai/agent_providers` is fork-owned BYOP routing and provider execution.
- `crates/warp_terminal` and `app/src/terminal` implement terminal emulation and product integration.
- `crates/warp_cli` and `app/src/local_control` provide local CLI and loopback control surfaces.

The app is deliberately accountless. Provider API keys stay in local managed-secret storage and provider OAuth must communicate directly with that provider. A wanted upstream feature coupled to Warp infrastructure must be adapted to a local implementation or explicitly deferred in the slice log; it must never regain a hidden Warp-server dependency.

### Entity-handle system

The global `App` owns model and view entities. Views refer to other entities through `ModelHandle<T>` and `ViewHandle<T>` rather than direct ownership. Context objects provide scoped access during updates, events, and rendering.

Create a `MouseStateHandle` once during construction and retain or clone it. Constructing a default handle during render breaks mouse interaction state.

## Engineering conventions

- Avoid redundant closure parameter type annotations.
- Consolidate imports at the top of a file; scoped imports are acceptable inside `#[cfg]` branches.
- Name application/view/model context parameters `ctx` and place them last, except that a closure parameter remains last.
- Remove unused parameters and update callers instead of prefixing names with `_`.
- Use inline format arguments such as `format!("{name}")`.
- Do not pass a single-use `Itertools::format` value to logging macros; materialize a reusable string first.
- Keep enum matches exhaustive; avoid `_` unless it is genuinely required.
- Preserve existing comments unless the changed behavior makes them inaccurate.
- A toggleable setting also needs its discoverable command-palette entry and required context flags.
- Spawn child processes through `crates/command`, never directly through `std::process::Command`.

### Terminal model locking

`TerminalModel::lock()` can deadlock the UI. Before adding a lock, verify that no caller already holds it. Prefer passing an already-locked reference down the call stack, keep lock scopes minimal, and do not call code that may reacquire the lock while it is held.

### Tests

Put unit tests in `${filename}_tests.rs` or `mod_test.rs` and include them from the implementation file:

```rust
#[cfg(test)]
#[path = "filename_tests.rs"]
mod tests;
```

Use `crates/integration` for integration tests. A test repair must preserve the product contract; do not merely weaken a newly failing assertion to make a merge gate green.

### Database

Persistence uses Diesel and SQLite. Add schema changes through a timestamped migration under `crates/persistence/migrations/`; do not hand-edit the generated schema as an independent change.

### Feature flags

Add flags to `crates/warp_core/src/features.rs` and the appropriate dogfood, preview, or release list. Prefer runtime `FeatureFlag::X.is_enabled()` checks; use `#[cfg]` only for platform or optional-dependency compilation. Gate the product entry point and implementation with the same flag, then remove shipped flags and dead branches.

## Upstream merge discipline

Warp is synchronized by real git merges, never by selective cherry-picking. Use the fixed slices and per-path decisions in `SPEC.md`.

- Always run git as `git -C <repo> ...` in automation.
- Fetch Warp with `--no-tags` to avoid release-tag pollution.
- Apply `script/upstream-strip.list` after each merge for recurring cloud tombstones.
- Preserve fork-owned Agent, CLI, SSH manager, BYOP, updater, and release code.
- Keep upstream product behavior, adapting only its Warp-cloud boundary.
- Never strip `app/src/ai/request_usage_model.rs`; it is the fork's local unlimited stub.
- The fork pins an older `warp_multi_agent_api`; omit proto-only orchestration conversions rather than upgrading it implicitly.
- Do not stage or commit a slice until all mandatory gates pass with zero new failing test names.
- Make one merge commit and one `zap-merge-slice-N` tag per accepted slice. Do not push unless the user explicitly requests it.

## Security and privacy boundary

Forbidden runtime dependencies include Warp login/auth, Warp server APIs, Warp GraphQL, subscriptions, billing, team entitlements, Warp Drive/cloud synchronization, session-sharing services, Firebase, telemetry, and Sentry.

Allowed outbound behavior is explicit product behavior: direct BYOP inference, direct provider OAuth, user-configured HTTPS endpoints, MCP servers selected by the user, and direct integrations such as the local `gh` CLI. Credentials must not be proxied through Warp or copied into logs.
