use itertools::Itertools;
use warpui::App;
use warpui::platform::{WindowBounds, WindowStyle};
use warpui::windowing::state::ApplicationStage;

use super::child_agent::{
    HiddenChildAgentConversationRequest, HiddenChildAgentTaskContext,
    create_hidden_child_agent_conversation,
};
use super::*;
use crate::notebooks::notebook::NotebookView;
use crate::terminal::shared_session::SharedSessionStatus;
use crate::test_util::assert_eventually;

// Zap: the upstream copy of this module registers its own list of singletons, most of them
// cloud models (server API, teams, IAP, shared-session manager, cloud environments, codebase
// index) that the fork removed. The workspace test initializer already registers everything a
// `PaneGroup` needs, so reuse it.
fn initialize_app(app: &mut App) {
    crate::workspace::view::tests::initialize_app(app);
}

struct MockOptions {
    layout: PanesLayout,
    window_bounds: WindowBounds,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self {
            layout: Default::default(),
            window_bounds: WindowBounds::ExactPosition(RectF::new(
                Vector2F::zero(),
                Vector2F::new(1024., 768.),
            )),
        }
    }
}

fn mock_pane_group(app: &mut App, options: MockOptions) -> ViewHandle<PaneGroup> {
    let tips_model = app.add_model(|_| TipsCompleted::default());
    let (_, pane_group) =
        app.add_window_with_bounds(WindowStyle::NotStealFocus, options.window_bounds, |ctx| {
            let user_default_shell_changed_banner_dismissal_model_handle =
                ctx.add_model(|_| BannerState::default());
            let block_lists = Arc::new(HashMap::new());
            PaneGroup::new_with_panes_layout(
                tips_model,
                user_default_shell_changed_banner_dismissal_model_handle,
                options.layout,
                block_lists,
                None,
                ctx,
            )
        });
    pane_group
}

fn get_newly_created_pane_id(panes: &PaneGroup, existing_ids: &[PaneId]) -> PaneId {
    panes
        .pane_ids()
        .find(|id| !existing_ids.contains(id))
        .unwrap()
}

fn split_pane_state(panes: &PaneGroup, pane_id: PaneId, ctx: &AppContext) -> SplitPaneState {
    panes
        .focus_state_handle()
        .as_ref(ctx)
        .split_pane_state_for(pane_id)
}

fn is_active_session(panes: &PaneGroup, pane_id: PaneId, ctx: &AppContext) -> bool {
    panes.active_session_id(ctx).map(Into::into) == Some(pane_id)
}

fn new_notebook(ctx: &mut ViewContext<PaneGroup>) -> ViewHandle<NotebookView> {
    ctx.add_typed_action_view(NotebookView::new)
}

fn new_ambient_agent_task_id() -> AmbientAgentTaskId {
    Uuid::new_v4().to_string().parse().unwrap()
}

fn start_parent_conversation(
    panes: &PaneGroup,
    parent_pane_id: PaneId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let parent_terminal_view_id = panes
        .terminal_view_from_pane_id(parent_pane_id, ctx)
        .expect("parent pane should have a terminal view")
        .id();
    start_parent_conversation_for_terminal_view(parent_terminal_view_id, ctx)
}

fn start_parent_conversation_for_terminal_view(
    terminal_view_id: EntityId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    BlocklistAIHistoryModel::handle(ctx).update(ctx, |history_model, ctx| {
        history_model.start_new_conversation(terminal_view_id, false, false, false, ctx)
    })
}

fn request_ambient_agent_task_id_for_hidden_child(
    panes: &PaneGroup,
    child_pane_id: PaneId,
    ctx: &mut ViewContext<PaneGroup>,
) -> Option<AmbientAgentTaskId> {
    let terminal_view = panes
        .terminal_view_from_pane_id(child_pane_id, ctx)
        .expect("child pane should have a terminal view");
    let ai_controller = terminal_view.as_ref(ctx).ai_controller().clone();

    ai_controller.update(ctx, |controller, _| controller.get_ambient_agent_task_id())
}

struct PreAttachReturnsFalsePane {
    pane_id: PaneId,
    pane_configuration: ModelHandle<PaneConfiguration>,
}

impl PreAttachReturnsFalsePane {
    fn new(ctx: &mut ViewContext<PaneGroup>) -> Self {
        Self {
            pane_id: PaneId::dummy_pane_id(),
            pane_configuration: ctx.add_model(|_ctx| PaneConfiguration::new("")),
        }
    }
}

impl pane::PaneContent for PreAttachReturnsFalsePane {
    fn id(&self) -> PaneId {
        self.pane_id
    }

    fn pre_attach(&self, _group: &PaneGroup, _ctx: &mut ViewContext<PaneGroup>) -> bool {
        false
    }

    fn attach(
        &self,
        _group: &PaneGroup,
        _focus_handle: focus_state::PaneFocusHandle,
        _ctx: &mut ViewContext<PaneGroup>,
    ) {
    }

    fn detach(
        &self,
        _group: &PaneGroup,
        _detach_type: pane::DetachType,
        _ctx: &mut ViewContext<PaneGroup>,
    ) {
    }

    fn snapshot(&self, _app: &AppContext) -> LeafContents {
        LeafContents::GetStarted
    }

    fn has_application_focus(&self, _ctx: &mut ViewContext<PaneGroup>) -> bool {
        false
    }

    fn focus(&self, _ctx: &mut ViewContext<PaneGroup>) {}

    fn shareable_link(
        &self,
        _ctx: &mut ViewContext<PaneGroup>,
    ) -> Result<pane::ShareableLink, pane::ShareableLinkError> {
        Ok(pane::ShareableLink::Base)
    }

    fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    fn is_pane_being_dragged(&self, _ctx: &AppContext) -> bool {
        false
    }
}

// TODO: This test is commented out for now until we can fix it. It is flaky and sometimes hangs, causing the CI to cancel.
// #[test]
// #[allow(clippy::clone_on_copy)]
// fn test_pane_history() {
//     App::test((), |mut app| async move {
//         let pane_group = mock_pane_group(&mut app, platform);

//         pane_group.update(&mut app, |panes, ctx| {
//             let mut entity_ids: Vec<EntityId> =
//                 panes.view_id_to_session_data.keys().cloned().collect();

//             let first_entity_id = entity_ids.get(0).unwrap().clone();

//             // Add pane Left.
//             panes.add_pane(Direction::Left, ctx);
//             entity_ids = panes.view_id_to_session_data.keys().cloned().collect();
//             entity_ids.retain(|x| *x != first_entity_id);
//             let second_entity_id = entity_ids.get(0).unwrap().clone();
//             // Add pane Up.
//             panes.add_pane(Direction::Up, ctx);
//             entity_ids = panes.view_id_to_session_data.keys().cloned().collect();
//             entity_ids.retain(|x| *x != first_entity_id && *x != second_entity_id);
//             let third_entity_id = entity_ids.get(0).unwrap().clone();

//             assert!(panes.prev_session_id(third_entity_id).unwrap() == second_entity_id);
//         })
//     });
// }

#[test]
#[allow(clippy::clone_on_copy)]
fn test_pane_focus_on_close() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first_pane_id = get_newly_created_pane_id(panes, &[]);

            // Add pane Left.
            panes.add_terminal_pane(Direction::Left, None, ctx);
            let second_pane_id = get_newly_created_pane_id(panes, &[first_pane_id]);

            assert!(panes.prev_pane_id(second_pane_id).unwrap() == first_pane_id);

            // Add pane Up.
            panes.add_terminal_pane(Direction::Up, None, ctx);
            let third_pane_id = get_newly_created_pane_id(panes, &[first_pane_id, second_pane_id]);

            // Close the third pane and check that the second pane opened is now focused.
            panes.close_pane(third_pane_id, ctx);
            assert_eq!(second_pane_id, panes.focused_pane_id(ctx));
        })
    });
}

#[test]
fn test_insert_hidden_child_agent_pane_keeps_focus_and_active_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let initial_tree_pane_count = panes.pane_count();
            let initial_content_pane_count = panes.pane_ids().count();
            let initial_visible_count = panes.visible_pane_count();
            let initial_active_session = panes.active_session_id(ctx);

            let child_pane_id = panes.insert_terminal_pane_hidden_for_child_agent(
                parent_pane_id,
                HashMap::new(),
                ctx,
            );

            assert_eq!(panes.pane_count(), initial_tree_pane_count);
            assert_eq!(panes.pane_ids().count(), initial_content_pane_count + 1);
            assert_eq!(panes.terminal_pane_ids().count(), 2);
            assert_eq!(panes.visible_pane_count(), initial_visible_count);
            assert!(panes.has_pane_id(child_pane_id.into()));
            assert!(!panes.panes.is_pane_in_tree(child_pane_id.into()));

            // The new child pane should remain off-tree and not affect visible ordering.
            assert_eq!(panes.pane_id_by_index(0), Some(parent_pane_id));
            assert_eq!(panes.pane_id_by_index(1), None);
            let visible_terminal_views = panes.visible_terminal_views(ctx);
            assert_eq!(visible_terminal_views.len(), 1);
            assert_eq!(
                visible_terminal_views[0].id(),
                panes
                    .terminal_view_from_pane_id(parent_pane_id, ctx)
                    .unwrap()
                    .id()
            );

            // Creating a hidden child pane should not steal focus or active session.
            assert_eq!(panes.focused_pane_id(ctx), parent_pane_id);
            assert_eq!(panes.active_session_id(ctx), initial_active_session);
        });
    });
}

#[test]
fn test_hidden_child_creation_applies_ambient_task_id_to_controller() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let task_id = new_ambient_agent_task_id();

            // Zap's local harness child launch passes a task context through this path.
            let child = create_hidden_child_agent_conversation(
                panes,
                HiddenChildAgentConversationRequest {
                    parent_pane_id,
                    name: "Agent 1".to_string(),
                    parent_conversation_id,
                    orchestration_harness: None,
                    env_vars: HashMap::new(),
                    task_context: Some(HiddenChildAgentTaskContext {
                        task_id,
                        working_dir: None,
                    }),
                },
                ctx,
            )
            .expect("fresh hidden child conversation should be created");

            let child_pane_id = panes
                .child_agent_panes
                .get(&child.conversation_id)
                .copied()
                .expect("fresh hidden child pane should be tracked");

            assert_eq!(
                request_ambient_agent_task_id_for_hidden_child(panes, child_pane_id, ctx,),
                Some(task_id)
            );
        });
    });
}

#[test]
fn test_active_session_id_reset_on_last_pane_close() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let terminal_id = get_newly_created_pane_id(panes, &[]);
            assert_eq!(
                panes.active_session_id(ctx),
                terminal_id.as_terminal_pane_id()
            );

            // Add a non-terminal pane (Notebook) so the pane group remains alive when terminal is closed.
            panes.add_pane_with_direction(
                Direction::Right,
                NotebookPane::new(new_notebook(ctx), ctx),
                false, /* focus_new_pane */
                ctx,
            );

            // Close the terminal.
            panes.close_pane(terminal_id, ctx);

            // active_session_id should be None after closing the last pane.
            assert_eq!(
                panes.active_session_id(ctx),
                None,
                "active_session_id should be None after closing the last pane"
            );
        });
    });
}

#[test]
fn test_add_pane_aborts_cleanly_when_pre_attach_returns_false() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let before_snapshot = panes.snapshot(ctx);
            let before_count = panes.pane_count();

            panes.add_pane_with_direction(
                Direction::Right,
                PreAttachReturnsFalsePane::new(ctx),
                true, /* focus_new_pane */
                ctx,
            );

            assert_eq!(panes.pane_count(), before_count);
            assert_eq!(panes.snapshot(ctx), before_snapshot);
        });
    });
}

#[test]
fn test_focus_notebook() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let first_terminal_id = get_newly_created_pane_id(panes, &[]);

            // Add a notebook to the left.
            panes.add_pane_with_direction(
                Direction::Left,
                NotebookPane::new(new_notebook(ctx), ctx),
                true, /* focus_new_pane */
                ctx,
            );
            let notebook_id = get_newly_created_pane_id(panes, &[first_terminal_id]);

            // The new pane should be focused, but the terminal is still the active session.
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(first_terminal_id)
            );
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert!(is_active_session(panes, first_terminal_id, ctx));
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );

            // Add a terminal below.
            panes.add_terminal_pane(Direction::Down, None, ctx);
            let second_terminal_id =
                get_newly_created_pane_id(panes, &[first_terminal_id, notebook_id]);

            // The new terminal should be both focused and the active session.
            assert_eq!(panes.focused_pane_id(ctx), second_terminal_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(second_terminal_id)
            );
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert!(!is_active_session(panes, first_terminal_id, ctx));
            assert_eq!(
                split_pane_state(panes, second_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );
            assert!(is_active_session(panes, second_terminal_id, ctx));
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );

            // Close the new terminal. Focus should switch to the notebook, and the first terminal
            // session will activate.
            panes.close_pane(second_terminal_id, ctx);
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(first_terminal_id)
            );
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );
            assert!(is_active_session(panes, first_terminal_id, ctx));
        })
    });
}

#[test]
fn test_group_without_terminals() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let terminal_id = get_newly_created_pane_id(panes, &[]);

            // Add a notebook to the left.
            panes.add_pane_with_direction(
                Direction::Left,
                NotebookPane::new(new_notebook(ctx), ctx),
                true, /* focus_new_pane */
                ctx,
            );
            let notebook_id = get_newly_created_pane_id(panes, &[terminal_id]);

            // Close the terminal, which should leave the group without an active session.
            panes.close_pane(terminal_id, ctx);
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(panes.active_session_id(ctx), None);
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::NotInSplitPane
            );
        });
    });
}

#[test]
fn test_close_active_session() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            // Add two terminal sessions.
            let first_terminal_id = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Up, None, ctx);
            let second_terminal_id = get_newly_created_pane_id(panes, &[first_terminal_id]);

            // Add a notebook to the left.
            panes.add_pane_with_direction(
                Direction::Left,
                NotebookPane::new(new_notebook(ctx), ctx),
                true, /* focus_new_pane */
                ctx,
            );
            let notebook_id =
                get_newly_created_pane_id(panes, &[first_terminal_id, second_terminal_id]);
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(second_terminal_id)
            );

            // Close the active session, which should leave the notebook focused and activate the
            // remaining session.
            panes.close_pane(second_terminal_id, ctx);
            assert_eq!(panes.focused_pane_id(ctx), notebook_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(first_terminal_id)
            );
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert!(is_active_session(panes, first_terminal_id, ctx));

            // Now, focus the remaining session, which should keep it activated.
            panes.focus_pane_by_id(first_terminal_id, ctx);
            assert_eq!(panes.focused_pane_id(ctx), first_terminal_id);
            assert_eq!(
                panes.active_session_id(ctx).map(Into::into),
                Some(first_terminal_id)
            );
            assert_eq!(
                split_pane_state(panes, first_terminal_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Focused)
            );
            assert_eq!(
                split_pane_state(panes, notebook_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Unfocused)
            );
            assert!(is_active_session(panes, first_terminal_id, ctx));
        });
    });
}

#[test]
fn test_update_session_visibility() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(&mut app, Default::default());
        pane_group.update(&mut app, |panes, ctx| {
            // Assert that there is no active window.
            WindowManager::handle(ctx).read(ctx, |state, _| {
                assert_eq!(state.stage(), ApplicationStage::Starting);
                assert!(state.active_window().is_none());
            });

            fn visibility_matches(panes: &PaneGroup, expected: bool, ctx: &ViewContext<PaneGroup>) {
                for data in panes.panes_of::<TerminalPane>() {
                    let view = data.terminal_view(ctx).as_ref(ctx);
                    assert_eq!(
                        view.was_ever_visible(),
                        expected,
                        "View {} visibility was {}, expected {}",
                        data.terminal_view(ctx).id(),
                        view.was_ever_visible(),
                        expected
                    );
                }
            }

            // Add pane Left.
            panes.add_terminal_pane(Direction::Left, None, ctx);

            // Assert that neither of the panes are marked as visible (due
            // to the fact that the window is not active).
            visibility_matches(panes, false, ctx);

            let window_id = ctx.window_id();
            WindowManager::handle(ctx).update(ctx, |state, ctx| {
                state.overwrite_for_test(ApplicationStage::Active, Some(window_id));
                ctx.notify();
            });

            // Assert that both of the panes are still not marked as
            // visible, given the fact that the pane group is not focused.
            visibility_matches(panes, false, ctx);

            panes.focus(ctx);

            // Assert that both of the panes are now visible.
            visibility_matches(panes, true, ctx);
        })
    });
}

#[test]
fn test_initial_widths_are_computed_correctly() {
    use launch_config::PaneTemplateType::*;

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Define a simple macro to help us create new leaf panes.
        macro_rules! leaf_pane {
            () => {
                PaneTemplate {
                    is_focused: None,
                    cwd: "".into(),
                    commands: vec![],
                    pane_mode: PaneMode::Terminal,
                    shell: None,
                }
            };
        }

        // Pick an arbitrary initial window that isn't the same as the
        // fallback value.
        let window_width = 864.;
        let window_height = 636.;
        assert_ne!(window_width, FALLBACK_INITIAL_WINDOW_SIZE.x());
        assert_ne!(window_height, FALLBACK_INITIAL_WINDOW_SIZE.y());

        // Create a template that looks like the following, with each pane
        // numbered by its index in the pane group:
        //
        //  ---------------------
        //  |         0         |
        //  | __________________|
        //  |     1   |____2____|
        //  | ________|____3____|
        //  |   4  |   5  |  6  |
        //  |      |      |     |
        //  ---------------------
        let template = PaneBranchTemplate {
            split_direction: launch_config::SplitDirection::Vertical,
            panes: vec![
                leaf_pane!(),
                PaneBranchTemplate {
                    split_direction: launch_config::SplitDirection::Horizontal,
                    panes: vec![
                        leaf_pane!(),
                        PaneBranchTemplate {
                            split_direction: launch_config::SplitDirection::Vertical,
                            panes: vec![leaf_pane!(), leaf_pane!()],
                        },
                    ],
                },
                PaneBranchTemplate {
                    split_direction: launch_config::SplitDirection::Horizontal,
                    panes: vec![leaf_pane!(), leaf_pane!(), leaf_pane!()],
                },
            ],
        };

        let window_size = Vector2F::new(window_width, window_height);
        let pane_group = mock_pane_group(
            &mut app,
            MockOptions {
                layout: PanesLayout::Template(template),
                window_bounds: WindowBounds::ExactPosition(RectF::new(
                    Vector2F::zero(),
                    window_size,
                )),
            },
        );

        // Assert that the window created by the call to `mock_pane_group`
        // has the expected bounds.
        let window_id = app.read(|ctx| pane_group.window_id(ctx));
        app.update(|ctx| {
            assert_eq!(
                Some(window_size),
                ctx.window_bounds(&window_id).map(|rect| rect.size())
            );
        });

        let pane_group_width = window_width - 2.0 * workspace::WORKSPACE_PADDING;
        let pane_group_height =
            window_height - workspace::TOTAL_TAB_BAR_HEIGHT - 2.0 * workspace::WORKSPACE_PADDING;

        pane_group.read(&app, |pane_group, ctx| {
            // Make assertions about the expected widths of the various
            // panes.
            assert_eq!(
                pane_group
                    .terminal_view_at_pane_index(0, ctx)
                    .unwrap()
                    .as_ref(ctx)
                    .size_info()
                    .pane_width_px()
                    .as_f32(),
                pane_group_width,
                "Pane with index 0 had unexpected width!"
            );
            let half_width = (pane_group_width - tree::get_divider_thickness()) / 2.;
            for i in 1..=3 {
                assert_eq!(
                    pane_group
                        .terminal_view_at_pane_index(i, ctx)
                        .unwrap()
                        .as_ref(ctx)
                        .size_info()
                        .pane_width_px()
                        .as_f32(),
                    half_width,
                    "Pane with index {i} had unexpected width!"
                );
            }
            let one_third_width = (pane_group_width - (2. * tree::get_divider_thickness())) / 3.;
            for i in 4..=6 {
                assert_eq!(
                    pane_group
                        .terminal_view_at_pane_index(i, ctx)
                        .unwrap()
                        .as_ref(ctx)
                        .size_info()
                        .pane_width_px()
                        .as_f32(),
                    one_third_width,
                    "Pane with index {i} had unexpected width!"
                );
            }

            // Make assertions about the expected heights of the various
            // panes.
            let one_third_height = (pane_group_height - (2. * tree::get_divider_thickness())) / 3.;
            for i in (0..=1).chain(4..=6) {
                assert_eq!(
                    pane_group
                        .terminal_view_at_pane_index(i, ctx)
                        .unwrap()
                        .as_ref(ctx)
                        .size_info()
                        .pane_height_px()
                        .as_f32(),
                    one_third_height,
                    "Pane with index {i} had unexpected height!"
                );
            }
            let one_sixth_height = (pane_group_height - (5. * tree::get_divider_thickness())) / 6.;
            for i in 2..=3 {
                assert_eq!(
                    pane_group
                        .terminal_view_at_pane_index(i, ctx)
                        .unwrap()
                        .as_ref(ctx)
                        .size_info()
                        .pane_height_px()
                        .as_f32(),
                    one_sixth_height,
                    "Pane with index {i} had unexpected height!"
                );
            }
        });
    });
}

#[test]
fn test_is_terminal_pane_being_shared() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let pane_group = mock_pane_group(&mut app, Default::default());
        pane_group.update(&mut app, |panes, ctx| {
            assert!(!panes.is_terminal_pane_being_shared(ctx));

            // Add another pane; the pane group should still be "unshared".
            panes.add_terminal_pane(Direction::Left, None, ctx);
            assert!(!panes.is_terminal_pane_being_shared(ctx));

            // Make one of the terminal panes shared. There is now at least one terminal pane being shared.
            panes
                .terminal_session_by_pane_index(0)
                .expect("terminal pane exists")
                .terminal_manager(ctx)
                .as_ref(ctx)
                .model()
                .lock()
                .set_shared_session_status(SharedSessionStatus::ActiveSharer);
            assert!(panes.is_terminal_pane_being_shared(ctx));
        });
    });
}

#[test]
fn test_number_of_shared_panes() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            // We have two terminal sessions. Neither is shared
            let first_pane_id = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Up, None, ctx);
            assert_eq!(panes.number_of_shared_sessions(ctx), 0);

            // Make one pane shared
            panes
                .terminal_manager(0, ctx)
                .unwrap()
                .as_ref(ctx)
                .model()
                .lock()
                .set_shared_session_status(SharedSessionStatus::ActiveSharer);
            assert_eq!(panes.number_of_shared_sessions(ctx), 1);

            // Make both panes shared
            panes
                .terminal_manager(1, ctx)
                .unwrap()
                .as_ref(ctx)
                .model()
                .lock()
                .set_shared_session_status(SharedSessionStatus::ActiveSharer);
            assert_eq!(panes.number_of_shared_sessions(ctx), 2);

            // Close a pane
            panes.close_pane(first_pane_id, ctx);
            assert_eq!(panes.number_of_shared_sessions(ctx), 1);
        });
    });
}

#[test]
fn test_navigation_skips_hidden_closed_panes() {
    let _guard = FeatureFlag::UndoClosedPanes.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            // Add second terminal to the right to create a horizontal pair
            panes.add_terminal_pane(Direction::Right, None, ctx);

            // Add third terminal; place it to the right of current focus
            panes.add_terminal_pane(Direction::Right, None, ctx);

            // Determine ordered visible panes by index 0..2
            let a = panes.pane_id_by_index(0).expect("pane 0 exists");
            let b = panes.pane_id_by_index(1).expect("pane 1 exists");
            let c = panes.pane_id_by_index(2).expect("pane 2 exists");

            // Focus C and confirm prev would be B when all are visible
            panes.focus_pane_by_id(c, ctx);
            assert_eq!(panes.prev_pane_id_navigation(c), Some(b));

            // Close B (it will be hidden for undo and excluded from visible navigation)
            panes.close_pane(b, ctx);

            // Now prev from C should skip B and go to A
            assert_eq!(panes.prev_pane_id_navigation(c), Some(a));

            // And next from A should skip B and go to C
            assert_eq!(panes.next_pane_id(a), Some(c));
        })
    });
}

// Ensures that we always show the pane header for terminal panes, regardless of split state.
#[test]
fn test_terminal_pane_headers() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        // There should be a single terminal pane to start and the pane header should not be shown.
        pane_group.read(&app, |pane_group, ctx| {
            assert_eq!(pane_group.pane_contents.len(), 1);

            let terminal_panes = pane_group.panes_of::<TerminalPane>().collect_vec();
            assert_eq!(terminal_panes.len(), 1);

            let pane_view = terminal_panes[0].pane_view();
            let header_visible = pane_view
                .as_ref(ctx)
                .header()
                .as_ref(ctx)
                .is_visible_in_pane_group();
            assert!(header_visible);
        });

        // Create a terminal split pane.
        pane_group.update(&mut app, |pane_group, ctx| {
            pane_group.add_terminal_pane(Direction::Left, None, ctx);
        });

        // There should be two terminal panes and they should both have the pane header.
        pane_group.read(&app, |pane_group, ctx| {
            assert_eq!(pane_group.pane_contents.len(), 2);

            let terminal_panes = pane_group.panes_of::<TerminalPane>().collect_vec();
            assert_eq!(terminal_panes.len(), 2);

            for terminal_pane in terminal_panes {
                let pane_view = terminal_pane.pane_view();
                assert!(
                    pane_view
                        .as_ref(ctx)
                        .header()
                        .as_ref(ctx)
                        .is_visible_in_pane_group()
                );
            }
        });

        // Close one of the panes; the remaining pane should still have a header.
        pane_group.update(&mut app, |pane_group, ctx| {
            pane_group.close_pane(pane_group.focused_pane_id(ctx), ctx);
        });

        pane_group.read(&app, |pane_group, ctx| {
            assert_eq!(pane_group.pane_contents.len(), 1);

            let terminal_panes = pane_group.panes_of::<TerminalPane>().collect_vec();
            assert_eq!(terminal_panes.len(), 1);

            let pane_view = terminal_panes[0].pane_view();
            assert!(
                pane_view
                    .as_ref(ctx)
                    .header()
                    .as_ref(ctx)
                    .is_visible_in_pane_group()
            );
        });

        // Create a non-terminal split pane. Terminal pane header remains visible.
        pane_group.update(&mut app, |pane_group, ctx| {
            pane_group.add_pane_with_direction(
                Direction::Left,
                NotebookPane::new(new_notebook(ctx), ctx),
                true, /* focus_new_pane */
                ctx,
            );
        });

        pane_group.read(&app, |pane_group, ctx| {
            assert_eq!(pane_group.pane_contents.len(), 2);

            let terminal_panes = pane_group.panes_of::<TerminalPane>().collect_vec();
            assert_eq!(terminal_panes.len(), 1);

            let pane_view = terminal_panes[0].pane_view();
            assert!(
                pane_view
                    .as_ref(ctx)
                    .header()
                    .as_ref(ctx)
                    .is_visible_in_pane_group()
            );
        });
    });
}

/// Tests that focusing two different panes in quick succession does not cause
/// an infinite loop of focus changes, as outlined in this PR's description:
/// https://github.com/warpdotdev/warp-internal/pull/8990
#[cfg_attr(windows, ignore = "TODO(CORE-3626)")]
#[test]
fn test_pane_focus_does_not_have_an_infinite_event_loop() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Create a pane group with two terminal panes that will fight for
        // focus.
        let mock_options = MockOptions {
            layout: PanesLayout::Template(PaneTemplateType::PaneBranchTemplate {
                split_direction: crate::launch_configs::launch_config::SplitDirection::Horizontal,
                panes: vec![
                    PaneTemplateType::PaneTemplate {
                        is_focused: Some(true),
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                    PaneTemplateType::PaneTemplate {
                        is_focused: None,
                        cwd: "/".into(),
                        commands: vec![],
                        pane_mode: PaneMode::Terminal,
                        shell: None,
                    },
                ],
            }),
            ..Default::default()
        };
        let pane_group = mock_pane_group(&mut app, mock_options);

        // The cycle requires that we are constantly trying to focus the input.
        // An active and long-running block causes focus to move to the
        // terminal instead of the input, so we need to wait until we've
        // finished bootstrapping to ensure no such block will exist.
        assert_eventually!(
            2000 => {
                let mut all_terminals_bootstrapped = true;
                pane_group.update(&mut app, |pane_group, ctx| {
                    pane_group.for_all_terminal_panes(|terminal_view, _ctx| {
                        let model = terminal_view.model.lock();
                        let active_block = model.block_list().active_block();
                        if active_block.bootstrap_stage() != crate::terminal::model::bootstrap::BootstrapStage::PostBootstrapPrecmd ||
                            active_block.is_active_and_long_running() {
                            all_terminals_bootstrapped = false;
                        }
                    }, ctx);
                });
                all_terminals_bootstrapped
            },
            "timed out after ~10s waiting for terminals to finish bootstrapping"
        );

        pane_group.update(&mut app, |pane_group, ctx| {
            // Switch panes twice in quick succession.  We want to make
            // sure the test terminates and doesn't get into an infinite
            // loop.
            pane_group.navigate_next_pane(ctx);
            pane_group.navigate_next_pane(ctx);
        });
    });
}

/// A view to help us react to focus changes and know that they were processed
/// synchronously, not asynchronously (via an Effect::Event).
struct FocusDetectionView {
    pane_group: ViewHandle<PaneGroup>,
    new_focused_pane_id: Option<PaneId>,
}

impl FocusDetectionView {
    fn new(pane_group: ViewHandle<PaneGroup>, ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_view(&pane_group, |me, pane_group, event, ctx| {
            let Event::OpenPromptEditor = event else {
                return;
            };
            // This event is enqueued by us after the `Focus` effect, and so
            // by the time we receive it, application focus will have been
            // moved to the second pane, and (crucially) the pane group should
            // have updated its internal state accordingly (which is what we're
            // asserting here).

            let new_focused_pane_id = me
                .new_focused_pane_id
                .expect("should have set this already");
            pane_group.read(ctx, |pane_group, ctx| {
                assert_eq!(pane_group.focused_pane_id(ctx), new_focused_pane_id);
                assert_eq!(
                    pane_group.active_session_id(ctx),
                    new_focused_pane_id.as_terminal_pane_id()
                );
            });
        });
        Self {
            pane_group,
            new_focused_pane_id: None,
        }
    }
}

impl Entity for FocusDetectionView {
    type Event = ();
}

impl View for FocusDetectionView {
    fn ui_name() -> &'static str {
        "FocusDetectionView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        ChildView::new(&self.pane_group).finish()
    }
}

impl TypedActionView for FocusDetectionView {
    type Action = ();
}

/// This test ensures that a change in application focus causes the pane group
/// focused pane to update synchronously, without needing to wait for effect
/// flushing to occur.
///
/// The goal is to avoid situations where a delayed response to application
/// focus changes leads to an infinite loop of focusing and re-focusing two
/// different panes.
#[test]
fn test_focused_pane_is_synchronized_with_application_focus() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Create a pane group with two terminal panes, so that we can move
        // focus and observe the effects.
        let panes_layout = PanesLayout::Template(PaneTemplateType::PaneBranchTemplate {
            split_direction: crate::launch_configs::launch_config::SplitDirection::Horizontal,
            panes: vec![
                PaneTemplateType::PaneTemplate {
                    is_focused: Some(true),
                    cwd: "/".into(),
                    commands: vec![],
                    pane_mode: PaneMode::Terminal,
                    shell: None,
                },
                PaneTemplateType::PaneTemplate {
                    is_focused: None,
                    cwd: "/".into(),
                    commands: vec![],
                    pane_mode: PaneMode::Terminal,
                    shell: None,
                },
            ],
        });

        let tips_model = app.add_model(|_| TipsCompleted::default());
        let (_, root_view) =
            app.add_window_with_bounds(WindowStyle::NotStealFocus, WindowBounds::Default, |ctx| {
                let user_default_shell_changed_banner_dismissal_model_handle =
                    ctx.add_model(|_| BannerState::default());
                let block_lists = Arc::new(HashMap::new());
                let pane_group = ctx.add_typed_action_view(|ctx| {
                    PaneGroup::new_with_panes_layout(
                        tips_model,
                        user_default_shell_changed_banner_dismissal_model_handle,
                        panes_layout,
                        block_lists,
                        None,
                        ctx,
                    )
                });

                FocusDetectionView::new(pane_group, ctx)
            });
        let pane_group = root_view.read(&app, |root_view, _ctx| root_view.pane_group.clone());

        let (focused_pane_id, active_session_id) = pane_group.read(&app, |pane_group, ctx| {
            (
                pane_group.focused_pane_id(ctx),
                pane_group.active_session_id(ctx),
            )
        });

        let second_pane_id = pane_group.read(&app, |pane_group, _ctx| {
            pane_group
                .pane_ids()
                .find(|pane_id| *pane_id != focused_pane_id)
                .expect("should have more than one pane")
        });

        // Verify that the "second" pane is not focused or active.
        assert_ne!(focused_pane_id, second_pane_id);
        assert_ne!(active_session_id, second_pane_id.as_terminal_pane_id());

        root_view.update(&mut app, |root_view, _ctx| {
            root_view.new_focused_pane_id = Some(second_pane_id);
        });

        pane_group.update(&mut app, |pane_group, ctx| {
            // First, request a change of application focus to the second
            // pane's terminal view.
            pane_group
                .terminal_view_from_pane_id(second_pane_id, ctx)
                .expect("second pane is a terminal pane")
                .update(ctx, |_terminal_view, ctx| {
                    ctx.focus_self();
                });

            // Second, emit an event on the pane group to trigger assertion
            // logic in the FocusDetectionView.  This event effect is enqueued after
            // the focus effect but before the focus effect is processed, meaning
            // it will observe any changes that occurred synchronously as part
            // of the focus effect but will _not_ observe any changes that result
            // from events dispatched during focus handling.
            //
            // We use `OpenPromptEditor` because we can be confident that
            // nothing else above may have emitted this event.
            //
            // IMPORTANT: This MUST be emitted in the same pane group update
            // during which we focus the terminal view, to ensure that the
            // effect queue doesn't get processed or further modified before we
            // enqueue this event on the effect queue.
            ctx.emit(Event::OpenPromptEditor);
        });
    });
}

/// APP-5243: closing a file pane only hides it while undo-close is available, and the same view is
/// reattached without reopening its file. Releasing the file on close would therefore leave a
/// restored pane rendering content that can never update again. The file is released only once the
/// pane is permanently discarded.
#[cfg(feature = "local_fs")]
#[test]
fn test_undo_close_keeps_a_file_pane_watching_its_file() {
    use warp_files::FileModel;

    let _undo_closed_panes = FeatureFlag::UndoClosedPanes.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        app.add_singleton_model(FileModel::new);
        let pane_group = mock_pane_group(&mut app, Default::default());

        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("notes.md");
        std::fs::write(&path, "# before").expect("write file");

        pane_group.update(&mut app, |panes, ctx| {
            let pane = FilePane::new(
                Some(LocalOrRemotePath::Local(path.clone())),
                None,
                None,
                ctx,
            );
            panes.add_pane_with_direction(Direction::Right, pane, true, ctx);
        });

        let (file_pane_id, file_view) = pane_group.read(&app, |panes, ctx| {
            panes
                .file_notebook_panes(ctx)
                .next()
                .expect("the file pane should exist")
        });

        // Let the read settle so the pane is fully loaded and watching.
        let loaded = file_view.update(&mut app, |view, ctx| {
            let file_id = view.file_id_for_test().expect("the file should be open");
            let future_handle = FileModel::as_ref(ctx)
                .get_future_handle(file_id)
                .expect("Loading future should be present");
            ctx.await_spawned_future(future_handle.future_id())
        });
        loaded.await;

        // Close the way the pane header's close button does, which is the path that reaches
        // `BackingView::close` before the pane group hides the pane.
        file_view.update(&mut app, BackingView::close);
        pane_group.update(&mut app, |panes, ctx| {
            assert!(
                panes.is_pane_hidden_for_close(file_pane_id),
                "closing should hide the pane for undo rather than discard it"
            );
            assert!(
                panes.restore_closed_pane(file_pane_id, ctx),
                "the closed pane should be restorable"
            );
        });

        app.read(|ctx| {
            let file_id = file_view
                .as_ref(ctx)
                .file_id_for_test()
                .expect("a restored pane should still hold its file open");
            assert!(
                FileModel::as_ref(ctx).file_path(file_id).is_some(),
                "a restored pane should still be tracked by the file model"
            );
        });

        // Permanently discarding the pane does release it.
        pane_group.update(&mut app, |panes, ctx| {
            panes.close_pane(file_pane_id, ctx);
            panes.cleanup_closed_pane(file_pane_id, ctx);
        });

        app.read(|ctx| {
            assert!(
                file_view.as_ref(ctx).file_id_for_test().is_none(),
                "a permanently discarded pane should release its file"
            );
        });
    });
}

// Upstream #10327 (swap-in-place child navigation), #10371 (lazy hidden-child
// restoration), #11776 (maximized state) and #15996 (reopened-tab ownership)
// coverage, restored for the ported swap model. Remote/ambient (cloud)
// variants are intentionally not carried: Zap's orchestration is local-only.

fn restore_conversation_for_terminal_view(
    terminal_view_id: EntityId,
    conversation: AIConversation,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let conversation_id = conversation.id();

    BlocklistAIHistoryModel::handle(ctx).update(ctx, |history_model, ctx| {
        history_model.restore_conversations(terminal_view_id, vec![conversation], ctx);
    });

    conversation_id
}

fn restore_child_conversation_for_terminal_view(
    terminal_view_id: EntityId,
    parent_conversation_id: AIConversationId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let mut child_conversation = AIConversation::new(false, false);
    child_conversation.set_parent_conversation_id(parent_conversation_id);
    restore_conversation_for_terminal_view(terminal_view_id, child_conversation, ctx)
}

fn restore_child_conversation(
    panes: &PaneGroup,
    pane_id: PaneId,
    parent_conversation_id: AIConversationId,
    ctx: &mut ViewContext<PaneGroup>,
) -> AIConversationId {
    let terminal_view_id = panes
        .terminal_view_from_pane_id(pane_id, ctx)
        .expect("child pane should have a terminal view")
        .id();
    restore_child_conversation_for_terminal_view(terminal_view_id, parent_conversation_id, ctx)
}

fn enter_agent_view_for_conversation(
    panes: &PaneGroup,
    pane_id: PaneId,
    conversation_id: AIConversationId,
    ctx: &mut ViewContext<PaneGroup>,
) {
    panes
        .terminal_view_from_pane_id(pane_id, ctx)
        .expect("pane should have a terminal view")
        .update(ctx, |terminal_view, ctx| {
            terminal_view.enter_agent_view_for_conversation(
                None,
                AgentViewEntryOrigin::RestoreExistingConversation,
                conversation_id,
                ctx,
            );
        });
}

fn create_already_fullscreen_parent_pane_data(
    panes: &PaneGroup,
    ctx: &mut ViewContext<PaneGroup>,
) -> (TerminalPane, PaneId, AIConversationId) {
    let (pane_data, terminal_view) =
        panes.create_terminal_pane_data(None, HashMap::new(), None, None, ctx);
    let pane_id = pane_data.terminal_pane_id().into();
    let parent_conversation_id =
        start_parent_conversation_for_terminal_view(terminal_view.id(), ctx);
    let child_conversation_id = restore_child_conversation_for_terminal_view(
        terminal_view.id(),
        parent_conversation_id,
        ctx,
    );

    terminal_view.update(ctx, |terminal_view, ctx| {
        terminal_view.enter_agent_view_for_conversation(
            None,
            AgentViewEntryOrigin::RestoreExistingConversation,
            parent_conversation_id,
            ctx,
        );
    });

    (pane_data, pane_id, child_conversation_id)
}

/// Creates a fresh local child agent (same path as a local `run_agents`
/// launch) under a new parent conversation in the group's first pane.
/// Returns `(parent_pane_id, parent_conversation_id, child_pane_id,
/// child_conversation_id)`.
fn create_parent_with_fresh_local_child(
    panes: &mut PaneGroup,
    ctx: &mut ViewContext<PaneGroup>,
) -> (PaneId, AIConversationId, PaneId, AIConversationId) {
    let parent_pane_id = get_newly_created_pane_id(panes, &[]);
    let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
    let child = create_hidden_child_agent_conversation(
        panes,
        HiddenChildAgentConversationRequest {
            parent_pane_id,
            name: "Agent 1".to_string(),
            parent_conversation_id,
            orchestration_harness: None,
            env_vars: HashMap::new(),
            task_context: None,
        },
        ctx,
    )
    .expect("fresh hidden child conversation should be created");
    let child_pane_id = panes
        .child_agent_panes
        .get(&child.conversation_id)
        .copied()
        .expect("fresh hidden child pane should be tracked");
    (
        parent_pane_id,
        parent_conversation_id,
        child_pane_id,
        child.conversation_id,
    )
}

#[test]
fn test_swapping_to_child_agent_from_maximized_pane_keeps_maximized_state() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            panes.add_terminal_pane(Direction::Right, None, ctx);
            panes.focus_pane(parent_pane_id, true, ctx);

            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let child = create_hidden_child_agent_conversation(
                panes,
                HiddenChildAgentConversationRequest {
                    parent_pane_id,
                    name: "Agent 1".to_string(),
                    parent_conversation_id,
                    orchestration_harness: None,
                    env_vars: HashMap::new(),
                    task_context: None,
                },
                ctx,
            )
            .expect("fresh hidden child conversation should be created");
            let child_pane_id = panes
                .child_agent_panes
                .get(&child.conversation_id)
                .copied()
                .expect("fresh hidden child pane should be tracked");

            panes.toggle_maximize_pane(ctx);
            assert!(panes.is_focused_pane_maximized(ctx));

            panes.swap_active_pane_to_conversation(parent_pane_id, child.conversation_id, ctx);

            assert_eq!(panes.focused_pane_id(ctx), child_pane_id);
            assert!(panes.is_focused_pane_maximized(ctx));
            assert_eq!(
                split_pane_state(panes, child_pane_id, ctx),
                SplitPaneState::InSplitPane(PaneState::Maximized),
            );
        });
    });
}

#[test]
fn test_restored_hidden_child_pane_reapplies_ambient_task_id_to_controller() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let task_id = new_ambient_agent_task_id();

            let mut child_conversation = AIConversation::new(false, false);
            child_conversation.set_parent_conversation_id(parent_conversation_id);
            child_conversation.set_task_id(task_id);
            // Zap: only local CLI-harness children are launched with a task
            // context, so only they get it back on restore.
            child_conversation.set_orchestration_harness(Harness::Codex);
            let child_conversation_id = child_conversation.id();

            panes.create_hidden_child_agent_pane(child_conversation, parent_pane_id, ctx);

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("restored hidden child pane should be tracked");

            assert_eq!(
                request_ambient_agent_task_id_for_hidden_child(panes, child_pane_id, ctx,),
                Some(task_id)
            );
        });
    });
}

/// Zap: a local Oz child's task id is the synthetic run id assigned at launch
/// (its own conversation id), and its launch never applies a task context, so
/// restoring it must not turn its requests into ambient-agent requests.
#[test]
fn test_restored_hidden_oz_child_pane_does_not_apply_synthetic_task_id() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);

            let mut child_conversation = AIConversation::new(false, false);
            child_conversation.set_parent_conversation_id(parent_conversation_id);
            child_conversation.set_task_id(new_ambient_agent_task_id());
            child_conversation.set_orchestration_harness(Harness::Oz);
            let child_conversation_id = child_conversation.id();

            panes.create_hidden_child_agent_pane(child_conversation, parent_pane_id, ctx);

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("restored hidden child pane should be tracked");

            assert_eq!(
                request_ambient_agent_task_id_for_hidden_child(panes, child_pane_id, ctx),
                None
            );
        });
    });
}

#[test]
fn test_pane_group_restore_loop_keeps_orchestration_topology_and_materializes_child_pane() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        let (
            parent_pane_id,
            parent_conversation_id,
            parent_run_id,
            child_conversation_id,
            child_run_id,
            child_agent_name,
        ) = pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_terminal_view_id = panes
                .terminal_view_from_pane_id(parent_pane_id, ctx)
                .expect("parent pane should have a terminal view")
                .id();

            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let parent_run_id = new_ambient_agent_task_id().to_string();
            let child_run_id = new_ambient_agent_task_id().to_string();
            let child_agent_name = "Agent 1".to_string();

            // Restore a child conversation into the parent's terminal view. This
            // is the same code path `RestoredAgentConversations::take_conversations`
            // feeds into during pane restoration.
            let mut child_conversation = AIConversation::new(false, false);
            child_conversation.set_parent_conversation_id(parent_conversation_id);
            child_conversation.set_agent_name(child_agent_name.clone());
            let child_conversation_id = child_conversation.id();
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.restore_conversations(
                    parent_terminal_view_id,
                    vec![child_conversation],
                    ctx,
                );
                // Stamp run_ids so orchestration agent_id lookups resolve.
                history.assign_run_id_for_conversation(
                    parent_conversation_id,
                    parent_run_id.clone(),
                    None,
                    parent_terminal_view_id,
                    ctx,
                );
                history.assign_run_id_for_conversation(
                    child_conversation_id,
                    child_run_id.clone(),
                    None,
                    parent_terminal_view_id,
                    ctx,
                );
            });

            (
                parent_pane_id,
                parent_conversation_id,
                parent_run_id,
                child_conversation_id,
                child_run_id,
                child_agent_name,
            )
        });

        // BEFORE the parent's fullscreen agent view is entered, the
        // orchestration data layer must already know the parent → child
        // topology, the child's local conversation (with agent name set), and
        // the child's run_id → conversation id mapping.
        pane_group.read(&app, |panes, ctx| {
            let history = BlocklistAIHistoryModel::as_ref(ctx);

            assert_eq!(
                history.child_conversation_ids_of(&parent_conversation_id),
                &[child_conversation_id],
                "orchestration topology must list the restored child under its parent before any pane materializes",
            );
            assert_eq!(
                crate::ai::blocklist::orchestration_topology::descendant_conversation_ids_in_spawn_order(
                    history,
                    parent_conversation_id,
                ),
                vec![child_conversation_id],
                "pill bar pre-order walker must reach the restored child before any pane materializes",
            );

            let child_conversation = history
                .conversation(&child_conversation_id)
                .expect("restored child must be in conversations_by_id before parent fullscreen");
            assert_eq!(
                child_conversation.agent_name(),
                Some(child_agent_name.as_str()),
                "restored child must retain its display name for transcript / pill bar rendering",
            );

            assert_eq!(
                history.conversation_id_for_agent_id(&child_run_id),
                Some(child_conversation_id),
                "child run_id must resolve to the restored child conversation",
            );
            assert_eq!(
                history.conversation_id_for_agent_id(&parent_run_id),
                Some(parent_conversation_id),
                "parent run_id must resolve to the parent conversation",
            );

            // Hidden child pane must NOT exist yet — restoration is lazy and
            // only materializes when the parent's agent view is entered.
            assert!(
                !panes.child_agent_panes.contains_key(&child_conversation_id),
                "hidden child pane must not exist before parent fullscreen entry",
            );
        });

        // Entering the parent's fullscreen agent view triggers
        // `restore_missing_child_agent_panes_for_parent`.
        pane_group.update(&mut app, |panes, ctx| {
            enter_agent_view_for_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
        });

        pane_group.read(&app, |panes, _ctx| {
            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("parent fullscreen entry must materialize the hidden child pane");
            assert!(
                panes.has_pane_id(child_pane_id),
                "materialized child pane must be tracked by the pane group",
            );
            assert!(
                !panes.panes.is_pane_in_tree(child_pane_id),
                "materialized child pane must remain off-tree (hidden)",
            );
        });
    });
}

#[test]
fn test_entering_parent_agent_view_lazily_restores_hidden_child_pane() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let (child_conversation_id, initial_pane_count, initial_visible_pane_count) = pane_group
            .update(&mut app, |panes, ctx| {
                let parent_pane_id = get_newly_created_pane_id(panes, &[]);
                let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
                let child_conversation_id =
                    restore_child_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
                let initial_pane_count = panes.pane_count();
                let initial_visible_pane_count = panes.visible_pane_count();

                assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));

                enter_agent_view_for_conversation(
                    panes,
                    parent_pane_id,
                    parent_conversation_id,
                    ctx,
                );
                (
                    child_conversation_id,
                    initial_pane_count,
                    initial_visible_pane_count,
                )
            });

        pane_group.update(&mut app, |panes, _ctx| {
            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("parent fullscreen restore should materialize the missing child pane");

            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
        });
    });
}

#[test]
fn test_add_pane_restores_hidden_child_when_parent_is_already_fullscreen() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let initial_pane_count = panes.pane_count();
            let initial_visible_pane_count = panes.visible_pane_count();
            let (pane_data, parent_pane_id, child_conversation_id) =
                create_already_fullscreen_parent_pane_data(panes, ctx);

            panes.add_pane_with_direction(Direction::Right, pane_data, true, ctx);

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("adding an already-fullscreen parent should materialize the child pane");

            assert!(panes.has_pane_id(parent_pane_id));
            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count + 1);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count + 1);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(panes.focused_pane_id(ctx), parent_pane_id);
            assert_eq!(
                panes.pane_id_for_owned_conversation(child_conversation_id, ctx),
                Some(child_pane_id)
            );
        });
    });
}

#[test]
fn test_reattach_panes_restores_hidden_child_when_parent_is_already_fullscreen() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let child_conversation_id =
                restore_child_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            let initial_pane_count = panes.pane_count();
            let initial_visible_pane_count = panes.visible_pane_count();

            panes.detach_panes(ctx);
            enter_agent_view_for_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));

            panes.reattach_panes(ctx);

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect(
                    "reattaching an already-fullscreen parent should materialize the child pane",
                );

            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(
                panes.pane_id_for_owned_conversation(child_conversation_id, ctx),
                Some(child_pane_id)
            );
        });
    });
}

#[test]
fn test_restore_closed_pane_restores_hidden_child_when_parent_is_already_fullscreen() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);
    let _undo_closed_panes = FeatureFlag::UndoClosedPanes.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            panes.add_pane_with_direction(
                Direction::Right,
                NotebookPane::new(new_notebook(ctx), ctx),
                false,
                ctx,
            );

            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let child_conversation_id =
                restore_child_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            let initial_pane_count = panes.pane_count();
            let initial_visible_pane_count = panes.visible_pane_count();

            panes.close_pane(parent_pane_id, ctx);
            assert!(panes.is_pane_hidden_for_close(parent_pane_id));

            enter_agent_view_for_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));

            assert!(panes.restore_closed_pane(parent_pane_id, ctx));

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect(
                    "restoring an already-fullscreen closed parent should materialize the child pane",
                );

            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(panes.focused_pane_id(ctx), parent_pane_id);
            assert_eq!(
                panes.pane_id_for_owned_conversation(child_conversation_id, ctx),
                Some(child_pane_id)
            );
        });
    });
}

#[test]
fn test_replace_pane_restores_hidden_child_when_replacement_is_already_fullscreen() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let original_pane_id = get_newly_created_pane_id(panes, &[]);
            let initial_pane_count = panes.pane_count();
            let initial_visible_pane_count = panes.visible_pane_count();
            let (replacement_pane, replacement_pane_id, child_conversation_id) =
                create_already_fullscreen_parent_pane_data(panes, ctx);

            assert!(panes.replace_pane(original_pane_id, replacement_pane, false, ctx));

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect(
                    "replacing with an already-fullscreen parent should materialize the child pane",
                );

            assert!(!panes.has_pane_id(original_pane_id));
            assert!(panes.has_pane_id(replacement_pane_id));
            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(panes.visible_pane_count(), initial_visible_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(panes.focused_pane_id(ctx), replacement_pane_id);
            assert_eq!(
                panes.pane_id_for_owned_conversation(child_conversation_id, ctx),
                Some(child_pane_id)
            );
        });
    });
}

#[test]
fn test_ensure_hidden_child_agent_pane_materializes_missing_child_pane() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
            let child_conversation_id =
                restore_child_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            let initial_pane_count = panes.pane_count();

            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));
            assert!(
                panes.ensure_hidden_child_agent_pane_for_conversation(child_conversation_id, ctx),
                "navigation fallback should materialize the missing child pane on demand"
            );

            let child_pane_id = panes
                .child_agent_panes
                .get(&child_conversation_id)
                .copied()
                .expect("on-demand ensure should track the restored child pane");
            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
        });
    });
}

#[test]
fn test_entering_parent_agent_view_skips_child_owned_by_another_pane() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());
        let (child_conversation_id, initial_pane_count) =
            pane_group.update(&mut app, |panes, ctx| {
                let parent_pane_id = get_newly_created_pane_id(panes, &[]);
                let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);

                panes.add_terminal_pane(Direction::Right, None, ctx);
                let sibling_pane_id = get_newly_created_pane_id(panes, &[parent_pane_id]);
                let child_conversation_id =
                    restore_child_conversation(panes, sibling_pane_id, parent_conversation_id, ctx);
                let initial_pane_count = panes.pane_count();

                enter_agent_view_for_conversation(
                    panes,
                    sibling_pane_id,
                    child_conversation_id,
                    ctx,
                );
                assert_eq!(
                    panes.pane_id_for_owned_conversation(child_conversation_id, ctx),
                    Some(sibling_pane_id)
                );

                enter_agent_view_for_conversation(
                    panes,
                    parent_pane_id,
                    parent_conversation_id,
                    ctx,
                );
                (child_conversation_id, initial_pane_count)
            });

        pane_group.update(&mut app, |panes, _ctx| {
            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
        });
    });
}

#[test]
fn test_ensure_hidden_child_agent_pane_skips_child_owned_by_another_pane_group() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let parent_pane_group = mock_pane_group(&mut app, Default::default());
        let other_pane_group = mock_pane_group(&mut app, Default::default());

        let parent_conversation_id = parent_pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = get_newly_created_pane_id(panes, &[]);
            start_parent_conversation(panes, parent_pane_id, ctx)
        });
        let (child_conversation_id, child_owner_terminal_view_id) =
            other_pane_group.update(&mut app, |panes, ctx| {
                let child_pane_id = get_newly_created_pane_id(panes, &[]);
                let child_conversation_id =
                    restore_child_conversation(panes, child_pane_id, parent_conversation_id, ctx);
                let initial_owner_terminal_view_id = panes
                    .terminal_view_from_pane_id(child_pane_id, ctx)
                    .expect("child pane should have a terminal view")
                    .id();

                enter_agent_view_for_conversation(panes, child_pane_id, child_conversation_id, ctx);
                (child_conversation_id, initial_owner_terminal_view_id)
            });

        parent_pane_group.update(&mut app, |panes, ctx| {
            let initial_pane_count = panes.pane_count();

            assert!(
                panes.ensure_hidden_child_agent_pane_for_conversation(child_conversation_id, ctx),
                "cross-tab child ownership should be treated as already reachable"
            );
            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(
                BlocklistAIHistoryModel::as_ref(ctx)
                    .terminal_surface_id_for_conversation(&child_conversation_id),
                Some(child_owner_terminal_view_id)
            );
        });
    });
}

#[test]
fn test_ensure_hidden_child_agent_pane_restores_child_from_detached_group() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let closed_pane_group = mock_pane_group(&mut app, Default::default());
        let reopened_pane_group = mock_pane_group(&mut app, Default::default());

        let (parent_conversation, child_conversation_id, previous_owner) = closed_pane_group
            .update(&mut app, |panes, ctx| {
                let parent_pane_id = panes.focused_pane_id(ctx);
                let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
                let mut child = AIConversation::new(false, false);
                child.set_parent_conversation_id(parent_conversation_id);
                child.set_agent_name("architect".to_string());
                let child_conversation_id = child.id();
                panes.create_hidden_child_agent_pane(child, parent_pane_id, ctx);

                let previous_owner = panes
                    .terminal_view_from_pane_id(
                        panes.child_agent_panes[&child_conversation_id],
                        ctx,
                    )
                    .unwrap();
                let parent_conversation = BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&parent_conversation_id)
                    .unwrap()
                    .clone();
                panes.swap_active_pane_to_conversation(parent_pane_id, child_conversation_id, ctx);
                panes.detach_panes(ctx);

                (parent_conversation, child_conversation_id, previous_owner)
            });

        let restored_child = reopened_pane_group.update(&mut app, |panes, ctx| {
            let parent_pane_id = panes.focused_pane_id(ctx);
            let parent_view = panes
                .terminal_view_from_pane_id(parent_pane_id, ctx)
                .unwrap();
            let parent_conversation_id =
                restore_conversation_for_terminal_view(parent_view.id(), parent_conversation, ctx);
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.set_active_conversation_id(parent_conversation_id, parent_view.id(), ctx);
            });

            assert!(
                panes.ensure_hidden_child_agent_pane_for_conversation(child_conversation_id, ctx)
            );
            let child_pane_id = *panes
                .child_agent_panes
                .get(&child_conversation_id)
                .expect("an undo-retained owner must not prevent restoring the child");
            panes.swap_active_pane_to_conversation(parent_pane_id, child_conversation_id, ctx);

            assert_eq!(panes.focused_pane_id(ctx), child_pane_id);
            let child_view = panes
                .terminal_view_from_pane_id(child_pane_id, ctx)
                .unwrap();
            assert_eq!(
                child_view.as_ref(ctx).active_conversation_id(ctx),
                Some(child_conversation_id)
            );
            child_view
        });

        assert_ne!(restored_child.id(), previous_owner.id());
        closed_pane_group.update(&mut app, |panes, ctx| {
            panes.reattach_panes(ctx);
            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));
            assert_eq!(
                panes.find_pane_id_for_terminal_view(previous_owner.id(), ctx),
                None
            );
            assert_eq!(panes.visible_pane_count(), 1);
            assert!(
                panes.ensure_hidden_child_agent_pane_for_conversation(child_conversation_id, ctx)
            );
        });
        closed_pane_group.update(&mut app, |panes, ctx| {
            panes.clean_up_panes(ctx);
        });
        app.read(|ctx| {
            assert_eq!(
                BlocklistAIHistoryModel::as_ref(ctx)
                    .terminal_surface_id_for_conversation(&child_conversation_id),
                Some(restored_child.id())
            );
            assert_eq!(
                restored_child.as_ref(ctx).active_conversation_id(ctx),
                Some(child_conversation_id)
            );
        });
    });
}

#[test]
fn test_entering_parent_agent_view_skips_child_owned_by_another_pane_group() {
    let _agent_view = FeatureFlag::AgentView.override_enabled(true);

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let parent_pane_group = mock_pane_group(&mut app, Default::default());
        let other_pane_group = mock_pane_group(&mut app, Default::default());

        let (parent_conversation_id, parent_pane_id) =
            parent_pane_group.update(&mut app, |panes, ctx| {
                let parent_pane_id = get_newly_created_pane_id(panes, &[]);
                let parent_conversation_id = start_parent_conversation(panes, parent_pane_id, ctx);
                (parent_conversation_id, parent_pane_id)
            });
        let (child_conversation_id, child_owner_terminal_view_id) =
            other_pane_group.update(&mut app, |panes, ctx| {
                let child_pane_id = get_newly_created_pane_id(panes, &[]);
                let child_conversation_id =
                    restore_child_conversation(panes, child_pane_id, parent_conversation_id, ctx);
                let initial_owner_terminal_view_id = panes
                    .terminal_view_from_pane_id(child_pane_id, ctx)
                    .expect("child pane should have a terminal view")
                    .id();

                enter_agent_view_for_conversation(panes, child_pane_id, child_conversation_id, ctx);
                (child_conversation_id, initial_owner_terminal_view_id)
            });
        let initial_pane_count = parent_pane_group.update(&mut app, |panes, ctx| {
            let initial_pane_count = panes.pane_count();
            enter_agent_view_for_conversation(panes, parent_pane_id, parent_conversation_id, ctx);
            initial_pane_count
        });

        parent_pane_group.update(&mut app, |panes, ctx| {
            assert!(!panes.child_agent_panes.contains_key(&child_conversation_id));
            assert_eq!(panes.pane_count(), initial_pane_count);
            assert_eq!(
                BlocklistAIHistoryModel::as_ref(ctx)
                    .terminal_surface_id_for_conversation(&child_conversation_id),
                Some(child_owner_terminal_view_id)
            );
        });
    });
}

/// Zap regression (broken since v2026.07.19.1): the orchestration pill bar,
/// the orchestrator status card and ESC back-navigation all end in the
/// terminal view's `RevealChildAgent` / `SwapPaneToConversation` events. The
/// pane group had no `SwapPaneToConversation` handler, so returning to the
/// orchestrator silently did nothing, and child reveals split the tab instead
/// of swapping in place. Drives both events through the real subscription.
#[test]
fn test_reveal_child_and_swap_back_events_swap_the_orchestrator_slot_in_place() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        let (parent_pane_id, parent_conversation_id, child_pane_id, child_conversation_id) =
            pane_group.update(&mut app, |panes, ctx| {
                let ids = create_parent_with_fresh_local_child(panes, ctx);
                let (parent_pane_id, _, _, child_conversation_id) = ids;
                panes
                    .terminal_view_from_pane_id(parent_pane_id, ctx)
                    .expect("parent pane should have a terminal view")
                    .update(ctx, |_, ctx| {
                        ctx.emit(crate::terminal::view::Event::RevealChildAgent {
                            conversation_id: child_conversation_id,
                        });
                    });
                ids
            });

        pane_group.update(&mut app, |panes, ctx| {
            // The child takes the orchestrator's slot instead of splitting the tab.
            assert_eq!(panes.focused_pane_id(ctx), child_pane_id);
            assert_eq!(panes.visible_pane_count(), 1);
            assert_eq!(
                panes.original_pane_for_replacement(child_pane_id),
                Some(parent_pane_id)
            );

            panes
                .terminal_view_from_pane_id(child_pane_id, ctx)
                .expect("child pane should have a terminal view")
                .update(ctx, |_, ctx| {
                    ctx.emit(crate::terminal::view::Event::SwapPaneToConversation {
                        conversation_id: parent_conversation_id,
                    });
                });
        });

        pane_group.read(&app, |panes, ctx| {
            assert_eq!(panes.focused_pane_id(ctx), parent_pane_id);
            assert_eq!(panes.visible_pane_count(), 1);
            assert!(panes.panes.is_pane_in_tree(parent_pane_id));
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(panes.original_pane_for_replacement(child_pane_id), None);
            // The child pane (and its live session) survives off-tree.
            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(
                panes.child_agent_panes.get(&child_conversation_id),
                Some(&child_pane_id)
            );
        });
    });
}

/// Zap regression: closing a swapped-in child returns it off-tree (not into
/// `hidden_panes`), and the next reveal re-hosts the same pane.
#[test]
fn test_closing_swapped_in_child_agent_pane_keeps_it_revealable() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        let (parent_pane_id, child_pane_id, child_conversation_id) =
            pane_group.update(&mut app, |panes, ctx| {
                let (parent_pane_id, _, child_pane_id, child_conversation_id) =
                    create_parent_with_fresh_local_child(panes, ctx);
                panes.swap_active_pane_to_conversation(parent_pane_id, child_conversation_id, ctx);
                assert_eq!(panes.focused_pane_id(ctx), child_pane_id);

                panes.close_pane(child_pane_id, ctx);

                assert_eq!(panes.focused_pane_id(ctx), parent_pane_id);
                assert_eq!(panes.visible_pane_count(), 1);
                assert!(panes.panes.is_pane_in_tree(parent_pane_id));
                assert!(!panes.panes.is_pane_in_tree(child_pane_id));
                assert!(!panes.panes.is_pane_hidden(&child_pane_id));
                assert!(panes.has_pane_id(child_pane_id));
                assert_eq!(
                    panes.child_agent_panes.get(&child_conversation_id),
                    Some(&child_pane_id)
                );

                panes
                    .terminal_view_from_pane_id(parent_pane_id, ctx)
                    .expect("parent pane should have a terminal view")
                    .update(ctx, |_, ctx| {
                        ctx.emit(crate::terminal::view::Event::RevealChildAgent {
                            conversation_id: child_conversation_id,
                        });
                    });
                (parent_pane_id, child_pane_id, child_conversation_id)
            });

        pane_group.read(&app, |panes, ctx| {
            assert_eq!(panes.focused_pane_id(ctx), child_pane_id);
            assert_eq!(
                panes.original_pane_for_replacement(child_pane_id),
                Some(parent_pane_id)
            );
            assert_eq!(
                panes.child_agent_panes.get(&child_conversation_id),
                Some(&child_pane_id)
            );
        });
    });
}

/// Zap regression: "Open in new pane" splits the existing child pane off next
/// to its orchestrator (reusing the live view), and closing it returns the
/// pane off-tree with the split-off marker cleared.
#[test]
fn test_open_child_agent_in_new_pane_event_splits_off_the_existing_child_pane() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let pane_group = mock_pane_group(&mut app, Default::default());

        let (parent_pane_id, child_pane_id, child_conversation_id) =
            pane_group.update(&mut app, |panes, ctx| {
                let (parent_pane_id, _, child_pane_id, child_conversation_id) =
                    create_parent_with_fresh_local_child(panes, ctx);
                panes
                    .terminal_view_from_pane_id(parent_pane_id, ctx)
                    .expect("parent pane should have a terminal view")
                    .update(ctx, |_, ctx| {
                        ctx.emit(crate::terminal::view::Event::OpenChildAgentInNewPane {
                            conversation_id: child_conversation_id,
                        });
                    });
                (parent_pane_id, child_pane_id, child_conversation_id)
            });

        pane_group.update(&mut app, |panes, ctx| {
            assert_eq!(panes.visible_pane_count(), 2);
            assert!(panes.panes.is_pane_in_tree(parent_pane_id));
            assert!(panes.panes.is_pane_in_tree(child_pane_id));
            assert_eq!(panes.original_pane_for_replacement(child_pane_id), None);
            assert_eq!(panes.focused_pane_id(ctx), child_pane_id);
            let child_view = panes
                .terminal_view_from_pane_id(child_pane_id, ctx)
                .expect("child pane should have a terminal view");
            assert!(child_view.as_ref(ctx).is_orchestration_split_off());

            panes.close_pane(child_pane_id, ctx);

            assert_eq!(panes.visible_pane_count(), 1);
            assert!(!panes.panes.is_pane_in_tree(child_pane_id));
            assert!(panes.has_pane_id(child_pane_id));
            assert_eq!(
                panes.child_agent_panes.get(&child_conversation_id),
                Some(&child_pane_id)
            );
            assert!(!child_view.as_ref(ctx).is_orchestration_split_off());
        });
    });
}
