use warp_core::ui::appearance::Appearance;
use warpui::platform::WindowStyle;
use warpui::{
    AddSingletonModel, App, EntityId, ModelHandle, SingletonEntity, ViewContext, ViewHandle,
};

use super::NotebookManager;
use crate::ai::blocklist::BlocklistAIHistoryModel;
use crate::auth::{AuthManager, AuthStateProvider};
use crate::cloud_object::Owner;
use crate::cloud_object::model::actions::ObjectActions;
use crate::cloud_object::model::persistence::ObjectStoreModel;
use crate::cloud_object::model::view::ObjectStoreViewModel;
use crate::cloud_object::update_manager::UpdateManager;
use crate::network::NetworkStatus;
use crate::notebooks::editor::keys::NotebookKeybindings;
use crate::notebooks::notebook::NotebookView;
use crate::pane_group::NotebookPane;
use crate::search::files::model::FileSearchModel;
use crate::settings::PrivacySettings;
use crate::settings_view::keybindings::KeybindingChangedNotifier;
use crate::terminal::keys::TerminalKeybindings;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::workspace::ActiveSession;
use crate::workspaces::user_profiles::UserProfiles;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::{GlobalResourceHandles, GlobalResourceHandlesProvider};

struct TestState {
    manager: ModelHandle<NotebookManager>,
}

impl TestState {
    /// Add a notebook view, configured by `init`, and register it with the [`NotebookManager`].
    fn add_notebook<F>(&self, app: &mut App, init: F) -> ViewHandle<NotebookView>
    where
        F: FnOnce(&mut NotebookView, &mut ViewContext<NotebookView>),
    {
        let (window, notebook) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut view = NotebookView::new(ctx);
            init(&mut view, ctx);
            view
        });

        self.manager.update(app, |manager, ctx| {
            let pane = NotebookPane::new(notebook.clone(), ctx);
            manager.register_pane(&pane, EntityId::new(), window, ctx)
        });

        notebook
    }
}

fn initialize_app(app: &mut App) -> TestState {
    initialize_settings_for_tests(app);

    let global_resources = GlobalResourceHandles::mock(app);
    app.add_singleton_model(|_| GlobalResourceHandlesProvider::new(global_resources));
    app.add_singleton_model(ObjectStoreModel::mock);
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| Appearance::mock());
    app.add_singleton_model(PrivacySettings::mock);
    app.add_singleton_model(UserWorkspaces::default_mock);
    app.add_singleton_model(|_| UserProfiles::new(vec![]));
    app.add_singleton_model(|_| ActiveSession::default());
    app.add_singleton_model(|_| ObjectActions::new(Vec::new()));
    app.add_singleton_model(|_| KeybindingChangedNotifier::new());
    app.add_singleton_model(|_| repo_metadata::repositories::DetectedRepositories::default());
    #[cfg(feature = "local_fs")]
    app.add_singleton_model(repo_metadata::RepoMetadataModel::new);
    app.add_singleton_model(FileSearchModel::new);
    app.add_singleton_model(NotebookKeybindings::new);
    app.add_singleton_model(TerminalKeybindings::new);
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AuthManager::new_for_test);
    app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
    #[cfg(feature = "voice_input")]
    app.add_singleton_model(voice_input::VoiceInput::new);

    app.add_singleton_model(|ctx| UpdateManager::new(None, ctx));
    // Zap (Wave 4): SyncQueue fully removed; the original `sync_queue.start_dequeueing(ctx)` no longer applies.

    app.add_singleton_model(ObjectStoreViewModel::mock);
    let manager = app.add_singleton_model(NotebookManager::mock);
    TestState { manager }
}

#[test]
fn test_save_on_close() {
    App::test((), |mut app| async move {
        let state = initialize_app(&mut app);
        let notebook = state.add_notebook(&mut app, |view, ctx| {
            view.open_new_notebook(
                Some("Test Notebook".to_string()),
                Owner::mock_current_user(),
                None,
                ctx,
            );
        });

        // Ensure the notebook has a pending edit.
        notebook.update(&mut app, |notebook, ctx| {
            notebook.input_editor().update(ctx, |editor, ctx| {
                editor.user_typed("Hello", ctx);
            });
        });

        let notebook_id = notebook.update(&mut app, |notebook, ctx| {
            notebook
                .notebook_id(ctx)
                .expect("notebook should have an id")
        });

        // Closing the notebook manager should trigger a save.
        state
            .manager
            .update(&mut app, |manager, ctx| manager.close_notebooks(ctx));

        app.read(|ctx| {
            let notebook = ObjectStoreModel::as_ref(ctx)
                .get_notebook(&notebook_id)
                .expect("notebook should remain in the local object store");
            assert_eq!(notebook.model().title, "Test Notebook");
            assert_eq!(notebook.model().data, "Hello");
        });
    });
}
