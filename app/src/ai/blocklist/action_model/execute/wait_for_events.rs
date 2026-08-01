//! Local watchdog for `AIAgentActionType::WaitForEvents`.

use std::collections::HashMap;
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use warpui::r#async::SpawnedFutureHandle;
use warpui::{Entity, ModelContext};

use super::{ActionExecution, AnyActionExecution, ExecuteActionInput};
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent::{AIAgentActionResultType, AIAgentActionType, WaitForEventsResult};

pub(crate) const DEFAULT_ORCHESTRATED_IDLE_TIMEOUT_SECONDS: i32 = 30 * 60;
pub(crate) const CLIENT_WATCHDOG_SAFETY_MARGIN: Duration = Duration::from_secs(30);
pub(crate) const HARD_FLOOR: Duration = Duration::from_secs(5);

pub(crate) fn watchdog_timeout_for_stamped_seconds(stamped_seconds: i32) -> Duration {
    let seconds = if stamped_seconds <= 0 {
        DEFAULT_ORCHESTRATED_IDLE_TIMEOUT_SECONDS
    } else {
        stamped_seconds
    };
    Duration::from_secs(seconds as u64)
        .checked_sub(CLIENT_WATCHDOG_SAFETY_MARGIN)
        .filter(|duration| *duration >= HARD_FLOOR)
        .unwrap_or(HARD_FLOOR)
}

struct PendingWait {
    tool_call_id: String,
    sender: async_channel::Sender<WaitForEventsResult>,
    watchdog_handle: SpawnedFutureHandle,
}

pub struct WaitForEventsExecutor {
    conversation_generation: HashMap<AIConversationId, usize>,
    pending: HashMap<AIConversationId, PendingWait>,
}

impl WaitForEventsExecutor {
    pub fn new() -> Self {
        Self {
            conversation_generation: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    pub(super) fn should_autoexecute(&self) -> bool {
        true
    }

    pub(super) fn preprocess_action(&mut self) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> impl Into<AnyActionExecution> + use<> {
        let AIAgentActionType::WaitForEvents(request) = &input.action.action else {
            return ActionExecution::InvalidAction;
        };

        let tool_call_id = request.tool_call_id.clone();
        let conversation_id = input.conversation_id;
        let timeout = watchdog_timeout_for_stamped_seconds(request.idle_timeout_seconds);
        let generation = self
            .conversation_generation
            .entry(conversation_id)
            .or_insert(0);
        *generation += 1;
        let expected_generation = *generation;

        let watchdog_tool_call_id = tool_call_id.clone();
        let watchdog_handle = ctx.spawn(
            async move {
                warpui::r#async::Timer::after(timeout).await;
            },
            move |me, (), _| {
                me.fire_watchdog_if_current(
                    conversation_id,
                    &watchdog_tool_call_id,
                    expected_generation,
                );
            },
        );

        let (sender, receiver) = async_channel::bounded(1);
        if let Some(previous) = self.pending.insert(
            conversation_id,
            PendingWait {
                tool_call_id,
                sender,
                watchdog_handle,
            },
        ) {
            previous.watchdog_handle.abort();
            drop(previous.sender);
        }

        ActionExecution::new_async(async move { receiver.recv().await }, |result, _| {
            AIAgentActionResultType::WaitForEvents(result.unwrap_or(WaitForEventsResult::Completed))
        })
    }

    pub(crate) fn complete_execution(&mut self, tool_call_id: &str) {
        let Some(conversation_id) = self.conversation_id_for_tool_call(tool_call_id) else {
            return;
        };
        let Some(pending) = self.pending.remove(&conversation_id) else {
            return;
        };
        pending.watchdog_handle.abort();
        let _ = pending.sender.try_send(WaitForEventsResult::Completed);
    }

    pub(crate) fn cancel_execution(&mut self, tool_call_id: &str) {
        let Some(conversation_id) = self.conversation_id_for_tool_call(tool_call_id) else {
            return;
        };
        let Some(pending) = self.pending.remove(&conversation_id) else {
            return;
        };
        if let Some(generation) = self.conversation_generation.get_mut(&conversation_id) {
            *generation += 1;
        }
        pending.watchdog_handle.abort();
        drop(pending.sender);
    }

    fn conversation_id_for_tool_call(&self, tool_call_id: &str) -> Option<AIConversationId> {
        self.pending.iter().find_map(|(conversation_id, pending)| {
            (pending.tool_call_id == tool_call_id).then_some(*conversation_id)
        })
    }

    fn fire_watchdog_if_current(
        &mut self,
        conversation_id: AIConversationId,
        tool_call_id: &str,
        expected_generation: usize,
    ) {
        if self
            .conversation_generation
            .get(&conversation_id)
            .copied()
            .unwrap_or_default()
            != expected_generation
        {
            return;
        }
        let Some(pending) = self.pending.get(&conversation_id) else {
            return;
        };
        if pending.tool_call_id != tool_call_id {
            return;
        }
        let Some(pending) = self.pending.remove(&conversation_id) else {
            return;
        };
        let _ = pending.sender.try_send(WaitForEventsResult::Completed);
    }
}

impl Entity for WaitForEventsExecutor {
    type Event = ();
}

#[cfg(test)]
#[path = "wait_for_events_tests.rs"]
mod tests;
