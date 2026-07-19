Spawns a local child agent that works on a sub-task in parallel while you continue working. The child runs in its own hidden conversation on this machine; you'll be notified via lifecycle events (progress, success, failure) as its state changes.

## When to use

- Genuinely parallelizable work: independent sub-tasks that don't touch the same files or state (e.g. fixing two unrelated test suites, researching one topic while you implement another).
- Long-running background work whose result you'll consume later.

## When NOT to use

- Work you can do directly with your own tools in a few steps — spawning has real overhead and the child starts with zero context.
- Sub-tasks that must share state with your current work (same files, same shell session): children run isolated, so coordinate through the filesystem only when tasks are truly independent.
- Never spawn a child just to run one shell command or read files.

## Rules

- The child **cannot see this conversation**. `prompt` must be complete and self-contained: include the goal, relevant file paths, constraints, and what "done" looks like.
- `name` is a short human-readable label shown in the UI (e.g. "Test Fixer", "Docs Writer").
- `harness` (optional) runs the child inside a third-party CLI agent installed on this machine: `claude` (Claude Code), `opencode`, or `codex`. Omit it to use the built-in native agent — the right default unless the user asked for a specific harness.
- Child agents cannot spawn their own children.
- At most a few children per turn are honored; prefer one or two well-scoped children over many small ones.
- Depending on the user's permission settings, each spawn may require explicit user approval — don't retry a spawn the user rejected.
