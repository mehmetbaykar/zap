use ai::skills::SkillPathOrigin;

use super::*;
use crate::ai::agent::task::TaskId;

fn convert_tool(tool: api::message::tool_call::Tool) -> MaybeAIAgentAction {
    let task_id = TaskId::new("task-1".to_string());
    let skill_path_origin = SkillPathOrigin::Local;
    let params = ConversionParams {
        task_id: &task_id,
        current_todo_list: None,
        active_code_review: None,
        skill_path_origin: &skill_path_origin,
    };
    api::message::ToolCall {
        tool_call_id: "call-1".to_string(),
        tool: Some(tool),
    }
    .to_action(params)
    .expect("conversion should not error")
}
