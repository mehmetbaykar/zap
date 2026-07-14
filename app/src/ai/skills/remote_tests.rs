use ai::skills::{ParsedSkill, SkillProvider, SkillScope};
use remote_server::proto::RemoteAgentContextSnapshot;
use tempfile::TempDir;
use warp_util::local_or_remote_path::LocalOrRemotePath;

use super::*;

fn daemon_skill(id: &str, content: &str) -> ParsedSkill {
    ParsedSkill {
        name: id.to_string(),
        description: format!("{id} description"),
        path: LocalOrRemotePath::Local(format!("/daemon/bundled/skills/{id}/SKILL.md").into()),
        content: content.to_string(),
        line_range: None,
        provider: SkillProvider::Zap,
        scope: SkillScope::Bundled,
    }
}

fn bundled_metadata(proto: &RemoteSkillProto) -> &BundledSkillMetadata {
    let Some(remote_skill_proto::Source::Bundled(metadata)) = proto.source.as_ref() else {
        panic!("expected bundled skill metadata");
    };
    metadata
}

#[test]
fn snapshot_protos_serialize_activation_conditions() {
    let temp_dir = TempDir::new().unwrap();
    let present_file = temp_dir.path().join("settings_schema.json");
    std::fs::write(&present_file, "{}").unwrap();
    let missing_file = temp_dir.path().join("missing.json");

    let catalog = BundledSkill::from_definitions([
        (
            "always-skill".to_string(),
            daemon_skill("always-skill", "# always"),
            BundledSkillActivation::Always,
        ),
        (
            "figma-skill".to_string(),
            daemon_skill("figma-skill", "# figma"),
            BundledSkillActivation::RequiresMcp(McpIntegration::Figma),
        ),
        (
            "file-present-skill".to_string(),
            daemon_skill("file-present-skill", "# file"),
            BundledSkillActivation::RequiresFile(present_file),
        ),
        (
            "file-missing-skill".to_string(),
            daemon_skill("file-missing-skill", "# missing"),
            BundledSkillActivation::RequiresFile(missing_file),
        ),
    ]);

    let protos = bundled_skill_snapshot_protos(&catalog);

    // `RequiresFile` is evaluated daemon-side: the missing-file skill is
    // dropped, the present-file skill ships as unconditionally active.
    let mut ids: Vec<&str> = protos
        .iter()
        .map(|proto| bundled_metadata(proto).id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, ["always-skill", "figma-skill", "file-present-skill"]);

    let figma = protos
        .iter()
        .find(|proto| bundled_metadata(proto).id == "figma-skill")
        .unwrap();
    assert_eq!(
        bundled_metadata(figma).requires_mcp.as_deref(),
        Some("figma")
    );
    for proto in protos
        .iter()
        .filter(|proto| bundled_metadata(proto).id != "figma-skill")
    {
        assert_eq!(bundled_metadata(proto).requires_mcp, None);
    }

    let always = protos
        .iter()
        .find(|proto| bundled_metadata(proto).id == "always-skill")
        .unwrap();
    assert_eq!(always.path, "/daemon/bundled/skills/always-skill/SKILL.md");
    assert_eq!(always.content, "# always");
}

#[test]
fn snapshot_protos_fit_aggregate_remote_agent_context_snapshot() {
    let catalog = BundledSkill::from_definitions([(
        "test-skill".to_string(),
        daemon_skill(
            "test-skill",
            "---\nname: test-skill\ndescription: A test skill\n---\nbody",
        ),
        BundledSkillActivation::Always,
    )]);

    let snapshot = RemoteAgentContextSnapshot {
        revision: 7,
        home_dir: "/remote/home".to_string(),
        skills: bundled_skill_snapshot_protos(&catalog),
        global_rules: Vec::new(),
    };

    assert_eq!(snapshot.skills.len(), 1);
    let skill = &snapshot.skills[0];
    assert_eq!(bundled_metadata(skill).id, "test-skill");
    assert_eq!(bundled_metadata(skill).requires_mcp, None);
    assert_eq!(skill.path, "/daemon/bundled/skills/test-skill/SKILL.md");
    assert_eq!(
        skill.content,
        "---\nname: test-skill\ndescription: A test skill\n---\nbody"
    );
}
