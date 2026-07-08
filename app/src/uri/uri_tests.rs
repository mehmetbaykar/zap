use super::*;
use crate::settings_view::settings_widget_deeplink_target;

// -- warp://settings deeplink parsing ----------------------------------------

#[test]
fn test_settings_widget_deeplink_target() {
    assert_eq!(
        settings_widget_deeplink_target("global_hotkey").map(|(section, _)| section),
        Some(SettingsSection::Features),
    );
    // custom_router / CustomModelRouters are not ported (Zap has no model-router).
    assert!(settings_widget_deeplink_target("custom_router").is_none());
    #[cfg(not(target_family = "wasm"))]
    assert_eq!(
        settings_widget_deeplink_target("cli_agents").map(|(section, _)| section),
        Some(SettingsSection::ThirdPartyCLIAgents),
    );
    // Unknown / empty slugs are not linkable (allowlist only).
    assert!(settings_widget_deeplink_target("not_a_widget").is_none());
    assert!(settings_widget_deeplink_target("").is_none());
}

#[test]
fn test_settings_section_for_simple_subpage() {
    assert_eq!(
        settings_section_for_simple_subpage("appearance"),
        Some(SettingsSection::Appearance),
    );
    assert_eq!(
        settings_section_for_simple_subpage("warp_agent"),
        Some(SettingsSection::WarpAgent),
    );
    // Zap stubs: billing / platform / teams are not simple settings sub-pages.
    assert!(settings_section_for_simple_subpage("billing_and_usage").is_none());
    assert!(settings_section_for_simple_subpage("platform").is_none());
    assert!(settings_section_for_simple_subpage("not_a_subpage").is_none());
}

// -- file:// open routing (upstream #9005 / #12866 / #13071) ------------------
// Exercised through the pure routing helper to avoid standing up a full
// `AppContext`.

#[test]
#[cfg(unix)]
fn test_open_file_executable_sh_routes_to_execute() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("run.sh");
    std::fs::write(&p, b"#!/bin/sh\n:\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    let action = classify_open_file_action(&p, true);
    assert_eq!(action, OpenFileAction::ExecuteInSession);
}

#[test]
#[cfg(unix)]
fn test_open_file_non_executable_sh_routes_to_editor() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("view.sh");
    std::fs::write(&p, b"#!/bin/sh\n:\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(classify_open_file_action(&p, true), OpenFileAction::Editor);
}

#[test]
#[cfg(unix)]
fn test_open_file_executable_bash_zsh_fish_route_to_execute() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    for name in ["run.bash", "run.zsh", "run.fish", "run.command"] {
        let p = dir.path().join(name);
        std::fs::write(&p, b"#!/bin/sh\n:\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            classify_open_file_action(&p, true),
            OpenFileAction::ExecuteInSession,
            "{name} should route to ExecuteInSession",
        );
    }
}

#[test]
fn test_open_file_markdown_routes_to_notebook_when_viewer_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("README.md");
    std::fs::write(&p, b"# hi\n").unwrap();
    assert_eq!(
        classify_open_file_action(&p, true),
        OpenFileAction::Notebook
    );
}

#[test]
fn test_open_file_markdown_routes_to_editor_when_viewer_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("README.md");
    std::fs::write(&p, b"# hi\n").unwrap();
    assert_eq!(classify_open_file_action(&p, false), OpenFileAction::Editor);
}

#[test]
fn test_open_file_ipynb_routes_to_notebook_when_enabled() {
    // A `.ipynb` opened via `file://` (e.g. "Open with Zap" from Finder) opens
    // in the notebook viewer, not the raw-JSON code editor — even when the
    // Markdown Viewer preference is disabled, which only governs Markdown.
    let _flag = crate::features::FeatureFlag::JupyterNotebookRendering.override_enabled(true);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("analysis.ipynb");
    std::fs::write(&p, b"{\"nbformat\": 4, \"cells\": []}\n").unwrap();
    assert_eq!(
        classify_open_file_action(&p, false),
        OpenFileAction::Notebook
    );
}

#[test]
fn test_open_file_ipynb_opens_in_editor_when_disabled() {
    // Without the feature flag, `.ipynb` is not rendered in the notebook viewer
    // and falls through to the code editor.
    let _flag = crate::features::FeatureFlag::JupyterNotebookRendering.override_enabled(false);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("analysis.ipynb");
    std::fs::write(&p, b"{\"nbformat\": 4, \"cells\": []}\n").unwrap();
    assert_eq!(classify_open_file_action(&p, true), OpenFileAction::Editor);
}

#[test]
#[cfg(feature = "local_fs")]
fn test_open_file_rust_source_still_opens_in_editor() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("main.rs");
    std::fs::write(&p, b"fn main() {}\n").unwrap();
    assert_eq!(classify_open_file_action(&p, true), OpenFileAction::Editor);
}

#[test]
fn test_open_file_directory_routes_to_session() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        classify_open_file_action(dir.path(), true),
        OpenFileAction::ExecuteInSession
    );
}

#[test]
#[cfg(unix)]
fn test_open_file_non_runnable_shebang_routes_to_editor() {
    // Extensionless `#!/bin/sh` file without the user-execute bit. Without the
    // shebang fall-through this would hit `ExecuteInSession` and the shell would
    // refuse to run it; the editor is the right place to view it.
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("noext");
    std::fs::write(&p, b"#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(classify_open_file_action(&p, true), OpenFileAction::Editor);
}
