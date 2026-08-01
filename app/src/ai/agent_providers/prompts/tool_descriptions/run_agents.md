Spawns a **batch** of local child agents in a single call. Every child in the batch shares one harness and one `base_prompt`, and each gets its own focused `prompt`; they all start at once and work in parallel while you continue. Each child runs in its own hidden conversation on this machine, and the tool result reports, per child, whether it launched (with its `agent_id`) or failed.

## When to use

- Fan-out work: two to four independent sub-tasks that share context but don't touch the same files or state (e.g. porting the same fix to four unrelated crates, auditing four subsystems against one checklist).
- You already know the whole batch up front. Deciding all the children in one call is what makes this cheaper than spawning them one at a time.
- Long-running background work whose results you'll collect later.

## When NOT to use

- A single sub-task — the batch machinery buys you nothing for one child.
- Work you can do directly with your own tools in a few steps: spawning has real overhead and every child starts with zero context.
- Sub-tasks that must share live state with your current work (same files, same shell session) or with each other: children run isolated and cannot coordinate, so only batch work that is genuinely independent.
- Never spawn children just to run shell commands or read files.

## Rules

- Children **cannot see this conversation**. `base_prompt` plus each `prompt` must be complete and self-contained: goal, relevant file paths, constraints, and what "done" looks like.
- `base_prompt` is prepended to every child's `prompt`. Put the shared context there and keep each `prompt` to that child's specific job — don't repeat the shared part.
- `summary` is one line describing the whole batch; it is shown to the user on the approval card.
- Each `agents[].name` must be non-empty and unique within the call — it is how the result correlates back to your request and how duplicate launches are rejected.
- `harness` (optional) applies to **every** child in the batch: `claude` (Claude Code), `opencode`, or `codex`, each of which must already be installed on this machine. Omit it to use the built-in native agent — the right default unless the user asked for a specific harness.
- At most 4 children per call, and at most 2 `run_agents` calls per assistant turn. Prefer one well-scoped batch over several small ones; wait for a batch to finish before launching the next.
- Child agents cannot spawn their own children.
- Depending on the user's permission settings, a batch may require explicit user approval before anything launches — don't re-issue a batch the user rejected.
