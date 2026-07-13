# APP-4717 — Enter on empty input sends the top queued prompt

See `specs/APP-4717/PRODUCT.md` for behavior. Researched at commit `e367c9de8b9629600885e40b029c10c8915f9ec8`.

## Context

- [`app/src/terminal/input.rs:12808 @ e367c9de`](https://github.com/warpdotdev/warp/blob/e367c9de8b9629600885e40b029c10c8915f9ec8/app/src/terminal/input.rs#L12808) — `Input::input_enter`. CLI-agent rich input returns early at the top (L12809-12867), so the queue-send path never applies there (PRODUCT §10). The new empty-buffer check runs before the existing queue/slash-command and shell-execution fallbacks; those existing branches require a non-empty buffer, so ordering is conflict-free.
- [`app/src/terminal/input.rs:3755-3793 @ e367c9de`](https://github.com/warpdotdev/warp/blob/e367c9de8b9629600885e40b029c10c8915f9ec8/app/src/terminal/input.rs#L3755-L3793) — `handle_queued_prompts_panel_event`: the existing Send-now dispatch (command vs prompt, `remove_fired_row`, refocus). This is the logic Enter must reuse.
- [`app/src/terminal/view/queued_prompts_panel.rs:580-620 @ e367c9de`](https://github.com/warpdotdev/warp/blob/e367c9de8b9629600885e40b029c10c8915f9ec8/app/src/terminal/view/queued_prompts_panel.rs#L580-L620) — `SendNow` action handler, which emits the selected row's id, text, and prompt/command kind for the host input to dispatch.
- [`app/src/terminal/view/queued_prompts_panel.rs:853-903 @ e367c9de`](https://github.com/warpdotdev/warp/blob/e367c9de8b9629600885e40b029c10c8915f9ec8/app/src/terminal/view/queued_prompts_panel.rs#L853-L903) — `render_header` ("N queued" label) where the "⏎ to send" hint goes. `should_render` (L548-563) already gates on flag, inline menus, and queue presence.
- [`app/src/terminal/input.rs:9756-9763 @ e367c9de`](https://github.com/warpdotdev/warp/blob/e367c9de8b9629600885e40b029c10c8915f9ec8/app/src/terminal/input.rs#L9756-L9763) — `Input` already detects empty↔non-empty buffer transitions on every `Edited` event (`is_editor_empty_on_last_edit`); the panel can be driven from here.

## Proposed changes

1. Shared dispatch helper on `Input` (`app/src/terminal/input.rs`): extract the body of the `QueuedPromptsPanelEvent::SendNow` arm into `fn send_queued_row_immediately(&mut self, conversation_id, query_id, text, is_command, ctx)`. Both the panel-event arm and the Enter path call it.
2. Panel send state (`app/src/terminal/view/queued_prompts_panel.rs`):
   - Host-input emptiness is *not* pushed: the panel holds the host editor's `ViewHandle` (passed at construction) and reads `is_empty` live at decision time, so the Enter decision cannot trail same-update buffer changes. A subscription to the host editor's `Edited`/`BufferReplaced` events re-renders the panel on empty↔non-empty transitions (a cached `host_editor_was_empty` flag only damps these notifications), mirroring the `CLIAgentSessionsModel` pattern below.
   The panel observes the CLI-agent rich input itself (a `CLIAgentSessionsModel` subscription plus a live `is_input_open` read — Enter submits to the CLI agent while it is open) and exposes `enter_sends_queued_prompt(ctx)` = `should_render` + live editor emptiness + rich input closed. The hint shows when that holds, no row is in inline edit mode, and the queue has a head row, so it cannot advertise an Enter that would not fire.
3. Enter path: inlined in `input_enter`'s dispatch chain: when `panel.enter_sends_queued_prompt(ctx)` holds, look up the head row of the active conversation's queue (`BlocklistAIHistoryModel::active_conversation_id` + `QueuedQueryModel::queue(...).first()`) and dispatch it via `send_queued_row_immediately`.
4. Header hint rendering: `render_header` appends an enter keycap chip (`render_keystroke_with_color_overrides`, the same component the "? for help" message-bar hints use) followed by "to send" text. The text uses the header's `sub_text_color`; the keycap glyph uses `internal_colors::text_disabled` so it is dimmer. Spacing follows the message-bar hint rules (`render_message_bar_items`): 8px label→keycap, 4px keycap→text.

No new feature flag: the behavior ships under the existing `QueueSlashCommand` gate the panel already requires.

## Testing and validation

- Unit tests in `app/src/terminal/input_tests.rs` next to the existing queued-panel host tests (L1277+), driving `input_enter`:
  - empty buffer + queued prompt row → head row dispatched, removed from queue, buffer untouched (PRODUCT §1, §11); a second Enter sends the next row (§3).
  - empty buffer + queued command row, default shell mode → command executed instead of an empty shell submission (§1, §2).
  - non-empty buffer → no queue send (§6).
  The flag-off case is intentionally not host-tested because with the flag off the panel (and hook target) does not exist.
- Panel tests in `app/src/terminal/view/queued_prompts_tests.rs`: hint hidden during inline edit and while the input is non-empty (§7, §9), while Send-now remains available. Panel tests reuse the host input's own panel when the flag is on — a second panel on the same terminal view would fight over edit-editor focus and commit edits on blur.
- `cargo check` + `./script/format`; manual smoke: queue two prompts during a running conversation, hit Enter twice with an empty input.

## Parallelization

Not beneficial: the change is small and tightly coupled (one host file + one panel file share the dispatch helper and the empty-state plumbing). A single agent implements it on this branch (`harry/app-4717-change-it-so-hitting-enter-w-an-empty-buffer-and-queued`).
