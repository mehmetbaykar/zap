//! [`TerminalView`] wiring for the pane-level conversation details panel.
//!
//! Zap: upstream keeps these helpers in `ambient_agent/view_impl.rs`, where they first try the
//! cloud `AmbientAgentTask` for the view's task id. This fork has no cloud task fetch, so only
//! the local `AIConversation` branch is kept, here, outside the ambient-agent module.

use warpui::{SingletonEntity, ViewContext};

use crate::ai::blocklist::BlocklistAIHistoryModel;
use crate::ai::conversation_details_panel::ConversationDetailsData;
use crate::terminal::view::TerminalView;

impl TerminalView {
    /// Updates the conversation details panel from the active local `AIConversation`
    /// of this terminal view, if any.
    pub(in crate::terminal::view) fn fetch_and_update_conversation_details_panel(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) {
        let view_id = self.id();
        let history_model = BlocklistAIHistoryModel::handle(ctx);
        let data = history_model
            .as_ref(ctx)
            .active_conversation(view_id)
            .map(|conversation| ConversationDetailsData::from_conversation(conversation, ctx));

        if let Some(data) = data {
            self.conversation_details_panel.update(ctx, |panel, ctx| {
                panel.set_conversation_details(data, ctx);
            });
        }
    }

    pub(in crate::terminal::view) fn refresh_conversation_details_panel_if_open(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.is_conversation_details_panel_open && self.can_show_conversation_details_ui(ctx) {
            self.fetch_and_update_conversation_details_panel(ctx);
            ctx.notify();
        }
    }
}
