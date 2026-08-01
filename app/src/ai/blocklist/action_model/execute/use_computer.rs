use ai::agent::action_result::AIAgentActionResultType;
use futures::FutureExt;
use futures::future::BoxFuture;
use warp_core::features::FeatureFlag;
use warpui::{Entity, ModelContext};

use super::{ActionExecution, AnyActionExecution, ExecuteActionInput, PreprocessActionInput};
use crate::ai::agent::{AIAgentActionType, UseComputerResult};

pub struct UseComputerExecutor;

impl UseComputerExecutor {
    pub fn new() -> Self {
        Self
    }

    pub(super) fn should_autoexecute(
        &self,
        input: ExecuteActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> bool {
        let ExecuteActionInput { action, .. } = input;
        let AIAgentActionType::UseComputer(_) = &action.action else {
            return false;
        };

        // This action is emitted only inside a computer-use subagent whose initial
        // RequestComputerUse action has already been approved.
        true
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> impl Into<AnyActionExecution> + use<> {
        let ExecuteActionInput { action, .. } = input;
        let AIAgentActionType::UseComputer(request) = &action.action else {
            return ActionExecution::InvalidAction;
        };

        let actions = request.actions.clone();
        let screenshot_params = request.screenshot_params;
        ActionExecution::new_async(
            async move {
                let mut actor = computer_use::create_actor();
                match actor
                    .perform_actions(
                        &actions,
                        computer_use::Options {
                            screenshot_params,
                            background_enabled: FeatureFlag::BackgroundComputerUse.is_enabled(),
                        },
                    )
                    .await
                {
                    Ok(result) => UseComputerResult::Success(result),
                    Err(error) => UseComputerResult::Error(error),
                }
            },
            |result, _ctx| AIAgentActionResultType::UseComputer(result),
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

impl Entity for UseComputerExecutor {
    type Event = ();
}
