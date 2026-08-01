use warp_core::features::FeatureFlag;
use warpui::{SingletonEntity, ViewContext};

use super::rich_content::RichContentMetadata;
use crate::ai::agent::CancellationReason;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::blocklist::block::{
    FinishReason, PendingUserQueryBlock, PendingUserQueryBlockEvent,
};
use crate::auth::AuthStateProvider;
use crate::terminal::TerminalView;

impl TerminalView {
    pub(super) fn pending_user_query_conversation_id(&self) -> Option<AIConversationId> {
        let view_id = self.pending_user_query_view_id?;
        self.rich_content_views
            .iter()
            .find(|rich_content| rich_content.view_id() == view_id)
            .and_then(|rich_content| rich_content.agent_view_conversation_id())
    }

    /// Inserts a pending user query block into the blocklist, showing the user that
    /// a follow-up query is queued and will be sent after the current conversation completes.
    /// `show_close_button` controls the dismiss ("X") button; `show_send_now_button` controls
    /// the "Send now" button that interrupts the active conversation and immediately submits
    /// the queued prompt.
    /// `locked_for_pending_lrc` marks pre-snapshot LRC auto-queue rows (upstream #13191).
    fn insert_pending_user_query_block(
        &mut self,
        prompt: String,
        show_close_button: bool,
        show_send_now_button: bool,
        locked_for_pending_lrc: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        // Preserve the queued-prompt callback across re-inserts used for unlock (re-render
        // with Send now enabled). `remove_pending_user_query_block` clears it.
        let preserved_callback = self.queued_prompt_callback.take();
        self.remove_pending_user_query_block(ctx);
        self.queued_prompt_callback = preserved_callback;

        let auth_state = AuthStateProvider::as_ref(ctx).get().clone();
        let user_display_name = auth_state
            .username_for_display()
            .unwrap_or_else(|| "User".to_owned());
        let profile_image_path = auth_state.user_photo_url();

        let prompt_for_send_now = prompt.clone();
        self.pending_user_query_prompt = Some(prompt.clone());
        self.pending_user_query_locked_for_lrc = locked_for_pending_lrc;

        let handle = ctx.add_typed_action_view(|ctx| {
            PendingUserQueryBlock::new(
                prompt,
                user_display_name,
                profile_image_path,
                show_close_button,
                show_send_now_button,
                ctx,
            )
        });
        ctx.subscribe_to_view(&handle, move |me, block, event, ctx| match event {
            PendingUserQueryBlockEvent::Dismissed => {
                if show_close_button {
                    me.remove_pending_user_query_block(ctx);
                }
            }
            PendingUserQueryBlockEvent::SendNow => {
                if show_send_now_button {
                    me.send_queued_prompt_now(prompt_for_send_now.clone(), ctx);
                }
            }
            PendingUserQueryBlockEvent::TextSelected => {
                // Ensure only one active text selection across the entire terminal view.
                me.clear_selected_text_except(Some(block.id()), ctx);
            }
        });
        let view_id = handle.id();

        self.insert_rich_content(
            None,
            handle.clone(),
            Some(RichContentMetadata::PendingUserQuery {
                pending_user_query_block_handle: handle,
            }),
            super::rich_content::RichContentInsertionPosition::PinToBottom,
            ctx,
        );
        self.pending_user_query_view_id = Some(view_id);
    }

    /// Inserts a pending user query block for a local ambient agent run, until the harness CLI starts.
    /// This block only shows the user prompt and queued status; it provides no local queue callback buttons.
    pub(in crate::terminal::view) fn insert_ambient_agent_queued_user_query_block(
        &mut self,
        prompt: String,
        ctx: &mut ViewContext<Self>,
    ) {
        self.insert_pending_user_query_block(
            prompt, /* show_close_button */ false, /* show_send_now_button */ false,
            /* locked_for_pending_lrc */ false, ctx,
        );
    }

    /// Removes the pending user query block, if one exists. No-op if none is present.
    /// Also cancels the queued prompt callback so the prompt is not sent.
    /// (Safe to call from within the callback itself — the caller `.take()`s it first.)
    pub(super) fn remove_pending_user_query_block(&mut self, ctx: &mut ViewContext<Self>) {
        self.queued_prompt_callback = None;
        self.pending_user_query_locked_for_lrc = false;
        self.pending_user_query_prompt = None;
        if let Some(view_id) = self.pending_user_query_view_id.take() {
            self.model
                .lock()
                .block_list_mut()
                .remove_rich_content(view_id);
            self.rich_content_views.retain(|rc| rc.view_id() != view_id);
            ctx.notify();
        }
    }

    /// Transitions a locked pending-LRC query to a normal interruptible queued prompt
    /// once the shell-command snapshot (or other FinishedAction) has fired.
    /// Mirrors upstream `QueuedQueryModel::unlock_pending_lrc_rows` (#13191).
    pub(super) fn unlock_pending_lrc_user_query(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.pending_user_query_locked_for_lrc {
            return;
        }
        let Some(prompt) = self.pending_user_query_prompt.clone() else {
            self.pending_user_query_locked_for_lrc = false;
            return;
        };
        // Re-insert with Send now enabled while preserving the auto-fire callback.
        self.insert_pending_user_query_block(
            prompt, /* show_close_button */ true, /* show_send_now_button */ true,
            /* locked_for_pending_lrc */ false, ctx,
        );
    }

    /// Removes the pending block and immediately submits the queued prompt.
    ///
    /// The plain-text submission path cancels any in-flight stream itself (via
    /// `send_query` -> `cancel_conversation_progress`), but slash- and skill-command
    /// submissions route through `send_request_input` directly without cancelling,
    /// which trips the in-flight-request assertion when the agent is still streaming.
    ///
    /// Cancel the active stream explicitly here so "Send now" works for any prompt type.
    /// Use `FollowUpSubmitted { is_for_same_conversation: true }` so the conversation
    /// status stays `InProgress` across the cancel+resend (see `mark_request_cancelled`
    /// in `conversation.rs`), keeping the warping indicator visible throughout.
    fn send_queued_prompt_now(&mut self, prompt: String, ctx: &mut ViewContext<Self>) {
        self.remove_pending_user_query_block(ctx);
        if let Some(conversation_id) = self
            .ai_context_model
            .as_ref(ctx)
            .selected_conversation_id(ctx)
        {
            self.ai_controller.update(ctx, |controller, ctx| {
                controller.cancel_conversation_progress(
                    conversation_id,
                    CancellationReason::FollowUpSubmitted {
                        is_for_same_conversation: true,
                    },
                    ctx,
                );
            });
        }

        self.input.update(ctx, |input, ctx| {
            input.submit_user_query_now(prompt, ctx);
        });
    }

    /// Shows a pending user query indicator and queues the query to be sent after
    /// the current conversation finishes. If the conversation completes successfully,
    /// the queued prompt is re-submitted through the normal input flow (so slash
    /// commands, skill commands, and session sharing are all handled correctly).
    /// The pending indicator is removed regardless of the finish reason.
    ///
    /// `show_close_button` controls whether a dismiss ("X") button appears on the pending
    /// block. `show_send_now_button` controls whether a "Send now" button appears that
    /// interrupts the active conversation and sends the queued prompt immediately. This
    /// should be false for summarization-triggered queuing (e.g. `/compact-and`) and for
    /// pre-snapshot LRC auto-queue (upstream #13191).
    /// `locked_for_pending_lrc` enables unlock-on-FinishedAction for the LRC race fix.
    pub fn send_user_query_after_next_conversation_finished(
        &mut self,
        prompt: String,
        show_close_button: bool,
        show_send_now_button: bool,
        locked_for_pending_lrc: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if FeatureFlag::PendingUserQueryIndicator.is_enabled() {
            self.insert_pending_user_query_block(
                prompt.clone(),
                show_close_button,
                show_send_now_button,
                locked_for_pending_lrc,
                ctx,
            );
        } else {
            // Still track lock state when the indicator flag is off so unlock/cancel
            // lifecycle stays consistent.
            self.pending_user_query_prompt = Some(prompt.clone());
            self.pending_user_query_locked_for_lrc = locked_for_pending_lrc;
        }
        // Replace any previously queued prompt so the latest one always wins.
        self.queued_prompt_callback = Some(Box::new(move |terminal_view, reason, ctx| {
            if FeatureFlag::PendingUserQueryIndicator.is_enabled() {
                terminal_view.remove_pending_user_query_block(ctx);
            } else {
                terminal_view.pending_user_query_locked_for_lrc = false;
                terminal_view.pending_user_query_prompt = None;
            }
            match reason {
                FinishReason::Complete => {
                    terminal_view.input.update(ctx, |input, ctx| {
                        input.submit_user_query_now(prompt, ctx);
                    });
                }
                FinishReason::Error
                | FinishReason::Cancelled
                | FinishReason::CancelledDuringRequestedCommandExecution => {
                    // Conversation failed or was cancelled — reinsert the pending
                    // query into the input so the user doesn't lose it.
                    terminal_view.input.update(ctx, |input, ctx| {
                        if input.buffer_text(ctx).is_empty() {
                            input.replace_buffer_content(&prompt, ctx);
                        }
                    });
                }
            }
        }));
    }
}
