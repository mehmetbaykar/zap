//! `read_skill`: reads Zap's Skill markdown templates.
//!
//! A Skill is a reusable workflow predefined by the user/project (a `SKILL.md` file + optional metadata).
//! After reading a skill, the model can advance the task following the steps the user expects. warp maintains a `SkillManager`
//! that indexes all available skills, referenceable by name (the frontmatter `name` field), absolute path, or
//! bundled id.
//!
//! ## Input contract
//!
//! The BYOP path exposes the `name` field, whose value is taken from the system prompt `<available_skills><skill><name>`.
//! `from_args` loads the name into the proto's `SkillReference::SkillPath` slot (without changing the proto),
//! and on a cache miss the `read_skill` executor first reverse-looks-up the real SKILL.md absolute path by name
//! and then reads from disk. This fallback also handles the case where the model directly passes an absolute path or the old bundled form
//! `@warp-skill:<id>`.
//!
//! ## Usage advice (written into the description)
//!
//! The model can proactively call it in the following scenarios:
//! - the user mentions a skill name / file name / path
//! - the task matches some skill's description (e.g. "do a PR review" triggers the `review` skill)

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use warp_multi_agent_api as api;

use super::OpenAiTool;

#[derive(Debug, Deserialize)]
struct Args {
    name: String,
}

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "The skill name (exactly matching the <available_skills><skill><name> field in the system prompt)."
            }
        },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn from_args(args: &str) -> Result<api::message::tool_call::Tool> {
    use api::message::tool_call::read_skill::SkillReference;
    let parsed: Args = serde_json::from_str(args)?;
    // reuse the proto's `SkillPath` slot to carry the name (to avoid a proto schema change);
    // on the executor side, on a cache miss it reverse-looks-up the real SKILL.md path by name.
    Ok(api::message::tool_call::Tool::ReadSkill(
        api::message::tool_call::ReadSkill {
            skill_reference: Some(SkillReference::SkillPath(parsed.name)),
            name: String::new(),
        },
    ))
}

fn result_to_json(result: &api::message::tool_call_result::Result) -> Option<Value> {
    use api::message::tool_call_result::Result as R;
    use api::read_skill_result::Result as SR;
    let r = match result {
        R::ReadSkill(r) => r,
        _ => return None,
    };
    let value = match &r.result {
        Some(SR::Success(s)) => {
            // FileContent { file_path, content, line_range } is directly a single message,
            // not a oneof, so there's no need to unwrap inner content.
            let (path, content) = s
                .content
                .as_ref()
                .map(|c| (c.file_path.clone(), c.content.clone()))
                .unwrap_or_default();
            json!({ "status": "ok", "path": path, "content": content })
        }
        Some(SR::Error(e)) => json!({ "status": "error", "message": e.message }),
        None => json!({ "status": "cancelled" }),
    };
    Some(value)
}

pub static READ_SKILL: OpenAiTool = OpenAiTool {
    name: "read_skill",
    description: include_str!("../prompts/tool_descriptions/read_skill.md"),
    parameters,
    from_args,
    result_to_json,
};
