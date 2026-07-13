use ai::agent::action_result::{AIAgentActionResultType, RequestComputerUseResult};
use futures::future::BoxFuture;
use futures::FutureExt;
use warpui::{Entity, EntityId, ModelContext, SingletonEntity};

use super::{ActionExecution, AnyActionExecution, ExecuteActionInput, PreprocessActionInput};
use crate::ai::agent::AIAgentActionType;

pub struct RequestComputerUseExecutor {
    terminal_view_id: EntityId,
}

impl RequestComputerUseExecutor {
    pub fn new(terminal_view_id: EntityId) -> Self {
        Self { terminal_view_id }
    }

    pub(super) fn should_autoexecute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let ExecuteActionInput { action, .. } = input;
        let AIAgentActionType::RequestComputerUse(_) = &action.action else {
            return false;
        };

        crate::ai::blocklist::BlocklistAIPermissions::as_ref(ctx)
            .get_computer_use_setting(ctx, Some(self.terminal_view_id))
            .is_always_allow()
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> impl Into<AnyActionExecution> {
        let ExecuteActionInput { action, .. } = input;
        let AIAgentActionType::RequestComputerUse(request) = &action.action else {
            return ActionExecution::InvalidAction;
        };

        let screenshot_params = request.screenshot_params;
        let mut actor = computer_use::create_actor();
        let platform = actor.platform();
        ActionExecution::new_async(
            async move {
                let result = actor
                    .perform_actions(&[], computer_use::Options { screenshot_params })
                    .await;
                (result, platform)
            },
            |action_result, _ctx| match action_result {
                (
                    Ok(computer_use::ActionResult {
                        screenshot: Some(screenshot),
                        ..
                    }),
                    Some(platform),
                ) => AIAgentActionResultType::RequestComputerUse(
                    RequestComputerUseResult::Approved {
                        screenshot,
                        platform,
                    },
                ),
                (
                    Ok(computer_use::ActionResult {
                        screenshot: Some(_),
                        ..
                    }),
                    None,
                ) => AIAgentActionResultType::RequestComputerUse(RequestComputerUseResult::Error(
                    "Unknown platform".to_string(),
                )),
                (Ok(_), _) => {
                    AIAgentActionResultType::RequestComputerUse(RequestComputerUseResult::Error(
                        "Failed to capture initial screenshot".to_string(),
                    ))
                }
                (Err(error), _) => AIAgentActionResultType::RequestComputerUse(
                    RequestComputerUseResult::Error(error),
                ),
            },
        )
    }

    pub(super) fn preprocess_action(
        &mut self,
        _input: PreprocessActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }
}

impl Entity for RequestComputerUseExecutor {
    type Event = ();
}
