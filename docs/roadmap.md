# Roadmap

Zap is **upstream Warp without the bullshit**: the full Warp product — terminal, agentic stack (agent loop, providers, MCP, skills, tool execution), and TUI — with every cloud, subscription, login, and telemetry dependency removed and replaced by local equivalents. We do not build a separate agent harness from scratch; upstream's is the best available, and our job is keeping it de-clouded and current.

Authoritative strategy and process: see `SPEC.md` (2026-07-10).

## Phase 1 — Catch-up merges (now)

- Restore the true git merge relationship with warpdotdev/warp: six fixed catch-up merge slices from merge-base `c325d146a` through pinned upstream tip `5e9dc1c24`, each buildable and gate-tested.
- All previously-skipped non-cloud features land via the merges (tab-groups, queued prompts, custom model routers, codebase auto-indexing, project rules, agent_sdk growth, `crates/mcp` structure).
- Cloud/subscription code stripped per the policy table; shims (`report_error!` → `log::error!`, auth gates → local constants) keep future upstream code compiling untouched.
- `warp_tui` enters in slice 5 as a non-default workspace member, with explicit `cargo check -p warp_tui` gates from that slice onward.

## Phase 2 — Steady state

- Weekly operator-initiated merge of upstream `main`; small conflict sets; baseline-diff test gate.
- zerx-lab/zap merged occasionally as a secondary source (their original fixes only; shared history dedupes).
- Provider-side OAuth (grok, harness CLIs) adapted provider↔app direct as upstream ships them.
- Test-debt burndown: drive the inherited `-p warp` failures to zero.

## Phase 3 — Local harness orchestration

- Adopt upstream's child-agent orchestration (Claude Code / Codex CLIs as local harnesses), availability gated on local CLI detection instead of Warp accounts.

## Principles (unchanged, ever)

- Local-only by default: credentials, history, skills, and MCP servers stay on disk.
- BYOP: bring-your-own-provider API keys or direct provider OAuth; no Warp account, no proxy through anyone's cloud.
- No telemetry.
