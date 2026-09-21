use std::sync::Arc;

use warp_util::standardized_path::StandardizedPath;
use warpui::App;
use warpui::platform::WindowStyle;

use super::*;
use crate::auth::AuthStateProvider;
use crate::terminal::model::session::Session;
use crate::test_util::settings::initialize_settings_for_tests;

struct TestView;

impl Entity for TestView {
    type Event = ();
}

impl View for TestView {
    fn ui_name() -> &'static str {
        "LocalSymbolContextMenuTest"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        Empty::new().finish()
    }
}

impl TypedActionView for TestView {
    type Action = ();
}

#[test]
fn local_code_category_works_in_agent_terminal_and_cli_inputs() {
    App::test((), |mut app| async move {
        let _flag = FeatureFlag::AIContextMenuCode.override_enabled(true);
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(|_| ActiveSession::default());
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let (window_id, _view) = app.add_window(WindowStyle::NotStealFocus, |_| TestView);
        WindowManager::handle(&app).update(&mut app, |windows, _| {
            windows.overwrite_for_test(windows.stage(), Some(window_id));
        });
        DetectedRepositories::handle(&app).update(&mut app, |repos, _| {
            repos.insert_test_repo_root(StandardizedPath::from_local_canonicalized(&root).unwrap());
        });
        let session = Arc::new(Session::test());
        ActiveSession::handle(&app).update(&mut app, |active, ctx| {
            active.set_session_for_test(window_id, session.clone(), Some(root), None, ctx);
        });

        app.read(|ctx| {
            assert!(*InputSettings::as_ref(ctx).outline_codebase_symbols_for_at_context_menu);
            for (ai_mode, cli_input) in [(true, false), (false, false), (true, true)] {
                let categories =
                    AIContextMenu::get_categories_for_mode(ai_mode, false, false, cli_input, ctx);
                assert!(categories.contains(&AIContextMenuCategory::Code));
                if cli_input || !ai_mode {
                    assert_eq!(
                        categories,
                        [
                            AIContextMenuCategory::RepoFiles,
                            AIContextMenuCategory::Code
                        ]
                    );
                }
            }
            assert!(
                !AIContextMenu::get_categories_for_mode(true, true, false, false, ctx)
                    .contains(&AIContextMenuCategory::Code)
            );
        });

        InputSettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .outline_codebase_symbols_for_at_context_menu
                .set_value(false, ctx)
                .unwrap();
        });
        app.read(|ctx| {
            assert!(
                !AIContextMenu::get_categories_for_mode(true, false, false, false, ctx)
                    .contains(&AIContextMenuCategory::Code)
            );
        });
    });
}

#[test]
fn code_category_is_not_advertised_without_a_local_repository() {
    App::test((), |mut app| async move {
        let _flag = FeatureFlag::AIContextMenuCode.override_enabled(true);
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(|_| ActiveSession::default());
        app.read(|ctx| {
            for cli_input in [false, true] {
                assert!(
                    !AIContextMenu::get_categories_for_mode(true, false, false, cli_input, ctx)
                        .contains(&AIContextMenuCategory::Code)
                );
            }
        });
    });
}
