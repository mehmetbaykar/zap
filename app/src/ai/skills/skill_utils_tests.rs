use std::collections::HashSet;
use std::path::PathBuf;

use ai::skills::{ParsedSkill, SkillProvider, SkillScope};
use repo_metadata::repositories::DetectedRepositories;
use repo_metadata::{DirectoryWatcher, RepoMetadataModel};
use warp_core::features::FeatureFlag;
use warp_util::host_id::HostId;
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warp_util::remote_path::RemotePath;
use warp_util::standardized_path::StandardizedPath;
use warpui::App;
use watcher::HomeDirectoryWatcher;

use super::*;
use crate::ai::skills::BundledSkillActivation;
use crate::warp_managed_paths_watcher::WarpManagedPathsWatcher;

fn remote_location(path: &str) -> LocalOrRemotePath {
    LocalOrRemotePath::Remote(RemotePath::new(
        HostId::new("remote-host".to_string()),
        StandardizedPath::try_new(path).unwrap(),
    ))
}

fn listed_skill_names(
    app: &App,
    working_directory: Option<&LocalOrRemotePath>,
    path_origin: &SkillPathOrigin,
) -> HashSet<String> {
    app.read(|ctx| {
        list_skills(working_directory, path_origin, ctx)
            .into_iter()
            .map(|skill| skill.name)
            .collect()
    })
}

/// Agent Mode lists skills from the active session's execution host: a remote session sees the
/// remote host's project and bundled skills (even before its working directory is known), never
/// the client's bundled catalog, and an unavailable remote session sees none.
#[test]
fn list_skills_uses_the_session_host_catalogs() {
    let host_id = HostId::new("remote-host".to_string());
    let remote_project_skill = ParsedSkill {
        name: "remote-project".to_string(),
        description: "remote project skill".to_string(),
        path: remote_location("/repo/.agents/skills/remote-project/SKILL.md"),
        content: "# remote-project".to_string(),
        line_range: None,
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    };
    let remote_bundled_skill = ParsedSkill {
        name: "remote-bundled".to_string(),
        description: "remote bundled skill".to_string(),
        path: remote_location("/opt/zap/resources/bundled/skills/remote-bundled/SKILL.md"),
        content: "# remote-bundled".to_string(),
        line_range: None,
        provider: SkillProvider::Zap,
        scope: SkillScope::Bundled,
    };
    let local_bundled_skill = ParsedSkill {
        name: "local-bundled".to_string(),
        description: "local bundled skill".to_string(),
        path: LocalOrRemotePath::Local(PathBuf::from("/bundled/skills/local-bundled/SKILL.md")),
        content: "# local-bundled".to_string(),
        line_range: None,
        provider: SkillProvider::Zap,
        scope: SkillScope::Bundled,
    };

    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
        app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
        let handle = app.add_singleton_model(SkillManager::new);
        let _bundled_skills = FeatureFlag::BundledSkills.override_enabled(true);

        handle.update(&mut app, |manager, ctx| {
            manager.handle_skills_added(vec![remote_project_skill], ctx);
            manager.add_bundled_skill_for_testing(
                "local-bundled",
                local_bundled_skill,
                BundledSkillActivation::Always,
            );
            manager.add_remote_bundled_skill_for_testing(
                host_id.clone(),
                "remote-bundled",
                remote_bundled_skill,
                BundledSkillActivation::Always,
            );
        });

        let remote_origin = SkillPathOrigin::Remote { host_id };
        let remote_names =
            listed_skill_names(&app, Some(&remote_location("/repo")), &remote_origin);
        assert!(remote_names.contains("remote-project"));
        assert!(remote_names.contains("remote-bundled"));
        assert!(!remote_names.contains("local-bundled"));

        let remote_names_without_cwd = listed_skill_names(&app, None, &remote_origin);
        assert!(remote_names_without_cwd.contains("remote-bundled"));
        assert!(!remote_names_without_cwd.contains("local-bundled"));

        let local_names = listed_skill_names(&app, None, &SkillPathOrigin::Local);
        assert!(local_names.contains("local-bundled"));
        assert!(!local_names.contains("remote-bundled"));
        assert!(!local_names.contains("remote-project"));

        assert!(listed_skill_names(&app, None, &SkillPathOrigin::Unavailable).is_empty());
    });
}

#[test]
fn skill_path_from_unix_encoded_remote_location() {
    let location = remote_location("/repo/.agents/skills/deploy/scripts/run.sh");

    assert_eq!(
        skill_path_from_location(&location),
        Some(remote_location("/repo/.agents/skills/deploy/SKILL.md"))
    );
}

#[test]
fn skill_path_from_windows_encoded_remote_location() {
    let location = remote_location(r"C:\repo\.agents\skills\deploy\scripts\run.ps1");

    assert_eq!(
        skill_path_from_location(&location),
        Some(remote_location(r"C:\repo\.agents\skills\deploy\SKILL.md"))
    );
}
#[test]
fn test_unique_skills_dedupes_identical_skills_same_dir() {
    let shared_skill_dir = PathBuf::from("/home/user");
    let skill_path1 = shared_skill_dir.join(".agents/skills/my-skill/SKILL.md");
    let skill_path2 = shared_skill_dir.join(".claude/skills/my-skill/SKILL.md");

    let content = "---\nname: test-skill\ndescription: A test skill\n---\nContent here";
    let skill = ParsedSkill {
        path: LocalOrRemotePath::Local(skill_path1.clone()),
        name: "test-skill".to_string(),
        description: "A test skill".to_string(),
        content: content.to_string(),
        line_range: Some(8..18),
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    };

    let skill2 = ParsedSkill {
        path: LocalOrRemotePath::Local(skill_path2.clone()),
        name: "test-skill".to_string(),
        description: "A test skill".to_string(),
        content: content.to_string(),
        line_range: Some(8..18),
        provider: SkillProvider::Claude,
        scope: SkillScope::Project,
    };

    let mut skills_by_path = HashMap::new();
    skills_by_path.insert(LocalOrRemotePath::Local(skill_path1.clone()), skill);
    skills_by_path.insert(LocalOrRemotePath::Local(skill_path2.clone()), skill2);

    let skill_paths = vec![
        (
            LocalOrRemotePath::Local(shared_skill_dir.clone()),
            LocalOrRemotePath::Local(skill_path1),
        ),
        (
            LocalOrRemotePath::Local(shared_skill_dir),
            LocalOrRemotePath::Local(skill_path2),
        ),
    ];

    let result = unique_skills(&skill_paths, &skills_by_path);
    assert_eq!(result.len(), 1);
    // Agents has higher priority (index 0) than Claude, so it should be preferred
    assert_eq!(result[0].provider, SkillProvider::Agents);
}

#[test]
fn test_unique_skills_keeps_same_provider_skills_from_different_dirs() {
    let home_dir = PathBuf::from("/home/user");
    let project_dir = PathBuf::from("/home/user/projects/repo");
    let home_path = home_dir.join(".agents/skills/my-skill/SKILL.md");
    let project_path = project_dir.join(".agents/skills/my-skill/SKILL.md");

    let content = "---\nname: test-skill\ndescription: A test skill\n---\nContent here";
    let home_skill = ParsedSkill {
        path: LocalOrRemotePath::Local(home_path.clone()),
        name: "test-skill".to_string(),
        description: "A test skill".to_string(),
        content: content.to_string(),
        line_range: Some(8..18),
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    };

    let project_skill = ParsedSkill {
        path: LocalOrRemotePath::Local(project_path.clone()),
        name: "test-skill".to_string(),
        description: "A test skill".to_string(),
        content: content.to_string(),
        line_range: Some(8..18),
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    };

    let mut skills_by_path = HashMap::new();
    skills_by_path.insert(LocalOrRemotePath::Local(home_path.clone()), home_skill);
    skills_by_path.insert(
        LocalOrRemotePath::Local(project_path.clone()),
        project_skill,
    );

    let skill_paths = vec![
        (
            LocalOrRemotePath::Local(home_dir),
            LocalOrRemotePath::Local(home_path.clone()),
        ),
        (
            LocalOrRemotePath::Local(project_dir),
            LocalOrRemotePath::Local(project_path.clone()),
        ),
    ];

    let result = unique_skills(&skill_paths, &skills_by_path);
    assert_eq!(
        result.len(),
        2,
        "same name + same provider across directories should each be kept"
    );
    assert!(
        result.iter().any(|skill| skill
            .reference
            .to_string()
            .contains(home_path.to_str().expect("home path is valid utf-8"))),
        "should keep the same-named skill in the home directory, actual={result:?}"
    );
    assert!(
        result.iter().any(|skill| skill
            .reference
            .to_string()
            .contains(project_path.to_str().expect("project path is valid utf-8"))),
        "should keep the same-named skill in the project directory, actual={result:?}"
    );
}

#[test]
fn test_unique_skills_name_dedup_same_name_different_providers() {
    let shared_skill_dir = PathBuf::from("/home/user");
    let skill_path1 = shared_skill_dir.join(".agents/skills/my-skill/SKILL.md");
    let skill_path2 = shared_skill_dir.join(".claude/skills/my-skill/SKILL.md");

    let content1 = "---\nname: test-skill\ndescription: A test skill\n---\nContent here";
    let content2 = "---\nname: test-skill\ndescription: A test skill\n---\nDifferent content";

    let skill1 = ParsedSkill {
        path: LocalOrRemotePath::Local(skill_path1.clone()),
        name: "test-skill".to_string(),
        description: "A test skill".to_string(),
        content: content1.to_string(),
        line_range: Some(8..18),
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    };

    let skill2 = ParsedSkill {
        path: LocalOrRemotePath::Local(skill_path2.clone()),
        name: "test-skill".to_string(),
        description: "A test skill".to_string(),
        content: content2.to_string(),
        line_range: Some(8..18),
        provider: SkillProvider::Claude,
        scope: SkillScope::Project,
    };

    let mut skills_by_path = HashMap::new();
    skills_by_path.insert(LocalOrRemotePath::Local(skill_path1.clone()), skill1);
    skills_by_path.insert(LocalOrRemotePath::Local(skill_path2.clone()), skill2);

    let skill_paths = vec![
        (
            LocalOrRemotePath::Local(shared_skill_dir.clone()),
            LocalOrRemotePath::Local(skill_path1),
        ),
        (
            LocalOrRemotePath::Local(shared_skill_dir),
            LocalOrRemotePath::Local(skill_path2),
        ),
    ];

    let result = unique_skills(&skill_paths, &skills_by_path);
    assert_eq!(
        result.len(),
        1,
        "same name, different content, different provider should be name-deduped, keeping only the highest-priority provider"
    );
    assert_eq!(
        result[0].provider,
        SkillProvider::Agents,
        "name-dedup should keep the higher-priority provider (Agents > Claude)"
    );
}
