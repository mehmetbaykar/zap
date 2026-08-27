use warp_core::features::FeatureFlag;
use warp_core::user_preferences::GetUserPreferences as _;
use warpui::platform::WindowStyle;
use warpui::{App, SingletonEntity, ViewHandle};

use super::{
    AuthOnboardingState, HAS_COMPLETED_ONBOARDING_KEY, NewWorkspaceSource, RootView,
    has_completed_local_onboarding,
};
use crate::GlobalResourceHandles;
use crate::ai::agent::api::ServerConversationToken;
use crate::auth::{AuthManager, AuthStateProvider};
use crate::workspace::Workspace;

fn initialize_app(app: &mut App) {
    app.update(crate::settings::init_and_register_user_preferences);
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AuthManager::new_for_test);
}

fn set_local_onboarding_completed(app: &mut App, completed: bool) {
    app.update(|ctx| {
        ctx.private_user_preferences()
            .write_value(
                HAS_COMPLETED_ONBOARDING_KEY,
                serde_json::to_string(&completed).unwrap(),
            )
            .unwrap();
    });
}

/// Regression test for the bug fixed by introducing
/// `RootView::finalize_local_onboarding_after_auth`: when a user completed onboarding
/// pre-login and later authenticated via a non-login-slide entrypoint (i.e. while
/// already in `Terminal` state), the server-side `is_onboarded` flag was never
/// flipped. The helper runs unconditionally on `AuthComplete` and must flip the
/// flag when all preconditions hold.
#[test]
fn test_sync_flips_server_is_onboarded_when_local_onboarding_completed() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Seed the "has_completed_local_onboarding" preference and make the user
        // appear not yet onboarded on the server. The default test user is
        // non-anonymous, so the guards in the helper won't short-circuit.
        set_local_onboarding_completed(&mut app, true);
        app.update(|ctx| {
            AuthStateProvider::as_ref(ctx).get().set_is_onboarded(false);
            assert!(has_completed_local_onboarding(ctx));
            assert_eq!(
                AuthStateProvider::as_ref(ctx).get().is_onboarded(),
                Some(false)
            );
        });

        app.update(|ctx| {
            let auth_state = AuthStateProvider::as_ref(ctx).get().clone();
            RootView::finalize_local_onboarding_after_auth(&auth_state, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                AuthStateProvider::as_ref(ctx).get().is_onboarded(),
                Some(true),
                "sync should have invoked AuthManager::set_user_onboarded"
            );
        });
    });
}

/// If the user hasn't completed local onboarding, the helper must leave the
/// server-side flag untouched — onboarding hasn't actually happened yet.
#[test]
fn test_sync_noop_when_local_onboarding_not_completed() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Do not set HAS_COMPLETED_ONBOARDING_KEY; it defaults to false.
        app.update(|ctx| {
            AuthStateProvider::as_ref(ctx).get().set_is_onboarded(false);
        });

        app.update(|ctx| {
            let auth_state = AuthStateProvider::as_ref(ctx).get().clone();
            RootView::finalize_local_onboarding_after_auth(&auth_state, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                AuthStateProvider::as_ref(ctx).get().is_onboarded(),
                Some(false),
                "sync should not have changed is_onboarded when local onboarding is incomplete"
            );
        });
    });
}

/// The server-side flag should also be left untouched when it is already set,
/// even if local onboarding is complete — avoids redundant server calls.
#[test]
fn test_sync_noop_when_already_onboarded_on_server() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        set_local_onboarding_completed(&mut app, true);
        app.update(|ctx| {
            // User::test() defaults to is_onboarded = true; assert that and
            // leave it in place.
            assert_eq!(
                AuthStateProvider::as_ref(ctx).get().is_onboarded(),
                Some(true)
            );
        });

        app.update(|ctx| {
            let auth_state = AuthStateProvider::as_ref(ctx).get().clone();
            RootView::finalize_local_onboarding_after_auth(&auth_state, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                AuthStateProvider::as_ref(ctx).get().is_onboarded(),
                Some(true)
            );
        });
    });
}

// Zap: upstream drives the content-deep-link tests below with a shared-session link
// (`NewWorkspaceSource::SharedSessionAsViewer`). Cloud session sharing was removed from the
// fork, so the persisted-conversation link is the content deep link that remains. A token
// that isn't in local history makes `Workspace::new` show the not-found toast and fall back
// to a default tab, which is all these tests need.
fn conversation_deep_link_source() -> NewWorkspaceSource {
    NewWorkspaceSource::FromCloudConversationId {
        conversation_id: ServerConversationToken::new(
            "root-view-test-conversation-token".to_string(),
        ),
    }
}

fn empty_workspace_source() -> NewWorkspaceSource {
    NewWorkspaceSource::Empty {
        previous_active_window: None,
        shell: None,
    }
}

fn root_view_with_source(app: &mut App, source: NewWorkspaceSource) -> ViewHandle<RootView> {
    let global_resource_handles = GlobalResourceHandles::mock(app);
    let (_, root_view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
        RootView::new(global_resource_handles, source, ctx)
    });
    root_view
}

/// Creates a workspace opened directly against a conversation deep link.
fn mock_workspace_from_content_deep_link(app: &mut App) -> ViewHandle<Workspace> {
    let global_resource_handles = GlobalResourceHandles::mock(app);
    let (_, workspace) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
        Workspace::new(
            global_resource_handles,
            None,
            conversation_deep_link_source(),
            ctx,
        )
    });
    workspace
}

/// A cold start straight into deep-linked content must bypass product onboarding.
///
/// The fork gates the pre-login slides on `ZapNewSettingsModes` as well as
/// `AgentOnboarding`, so both are forced on here.
#[test]
fn root_view_new_skips_onboarding_for_content_deep_link_cold_start() {
    let _agent_onboarding = FeatureFlag::AgentOnboarding.override_enabled(true);
    let _settings_modes = FeatureFlag::ZapNewSettingsModes.override_enabled(true);

    App::test((), |mut app| async move {
        crate::workspace::view::tests::initialize_app(&mut app);
        set_local_onboarding_completed(&mut app, false);

        // Control: with the flags on and local onboarding incomplete, a plain window still
        // enters onboarding, so the deep-link assertion below isn't vacuous.
        let plain_root_view = root_view_with_source(&mut app, empty_workspace_source());
        app.read(|ctx| {
            assert!(
                matches!(
                    plain_root_view.as_ref(ctx).auth_onboarding_state,
                    AuthOnboardingState::Onboarding { .. }
                ),
                "a plain cold start should still enter onboarding"
            );
        });

        let root_view = root_view_with_source(&mut app, conversation_deep_link_source());
        app.read(|ctx| {
            assert!(
                !matches!(
                    root_view.as_ref(ctx).auth_onboarding_state,
                    AuthOnboardingState::Onboarding { .. }
                ),
                "a cold-started content deep link window must not enter onboarding"
            );
        });
    });
}

#[test]
fn onboarding_slides_skip_content_deep_link_terminal() {
    App::test((), |mut app| async move {
        crate::workspace::view::tests::initialize_app(&mut app);
        let deep_link_workspace = mock_workspace_from_content_deep_link(&mut app);
        let plain_workspace = crate::workspace::view::tests::mock_workspace(&mut app);

        let root_view = root_view_with_source(&mut app, empty_workspace_source());

        root_view.update(&mut app, |root_view, ctx| {
            root_view.auth_onboarding_state =
                AuthOnboardingState::Terminal(deep_link_workspace.clone());
            root_view
                .auth_onboarding_state
                .try_open_onboarding_slides(ctx);
        });
        app.read(|ctx| {
            let AuthOnboardingState::Terminal(workspace) =
                &root_view.as_ref(ctx).auth_onboarding_state
            else {
                panic!("a content deep link workspace must not be wrapped in onboarding");
            };
            assert_eq!(
                workspace.id(),
                deep_link_workspace.id(),
                "a content deep link workspace must not be replaced by onboarding"
            );
        });

        root_view.update(&mut app, |root_view, ctx| {
            root_view.auth_onboarding_state =
                AuthOnboardingState::Terminal(plain_workspace.clone());
            root_view
                .auth_onboarding_state
                .try_open_onboarding_slides(ctx);
        });
        app.read(|ctx| {
            assert!(
                matches!(
                    root_view.as_ref(ctx).auth_onboarding_state,
                    AuthOnboardingState::Onboarding { .. }
                ),
                "a workspace opened with no content deep link should still get onboarding"
            );
        });
    });
}
