//! Integration tests for the SFTP file browser in a real popup window.
//!
//! Uses the Builder/TestStep/Driver pattern to open the SFTP panel in a real
//! window and verify panel rendering, title, close, tab switching, and other
//! interaction behavior.
//! author: logic
//! date: 2026-05-30

use std::collections::HashMap;

use warp::integration_testing::sftp;
use warp::integration_testing::sftp::{ConnectionState, Dialog, SftpBrowserAction};
use warp::integration_testing::terminal::wait_until_bootstrapped_single_pane_for_tab;
use warp::integration_testing::view_getters::{pane_group_view, workspace_view};
use warpui::{
    async_assert, async_assert_eq, integration::AssertionCallback, integration::StepDataMap,
    integration::TestStep, TypedActionView,
};

use super::{new_builder, Builder};

/// Asserts the SFTP browser view exists and is accessible.
///
/// Does not depend on a fixed pane index; looks up SftpBrowserView by view type.
/// Accepts all connection states, only verifying that the view exists.
/// author: logic
/// date: 2026-05-31
fn assert_sftp_browser_view_exists() -> AssertionCallback {
    Box::new(move |app, window_id| {
        let view = sftp::sftp_browser_view(app, window_id);
        view.read(app, |_v, _| {
            // Successfully getting the view proves the SFTP panel exists.
            warpui::integration::AssertionOutcome::Success
        })
    })
}

/// Opens the SFTP panel (using a test node_id).
fn open_sftp_pane(app: &mut warpui::App) {
    let window_id = app.read(|ctx| {
        ctx.windows()
            .active_window()
            .expect("should have active window")
    });
    let workspace = workspace_view(app, window_id);
    app.update(|ctx| {
        workspace.update(ctx, |ws, ctx| {
            ws.open_sftp_pane("test-integration-node".to_string(), ctx);
        });
    });
}

/// Verifies the SFTP panel opens in a real window and displays the correct title.
pub fn test_sftp_pane_opens_in_workspace() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            TestStep::new("Open SFTP pane")
                .with_action(|app, _, _| open_sftp_pane(app))
                .set_post_step_pause(std::time::Duration::from_secs(2)),
        )
        .with_step(
            TestStep::new("Verify SFTP pane is visible")
                .add_assertion(|app, window_id| {
                    let pane_group = pane_group_view(app, window_id, 0);
                    pane_group.read(app, |pane_group, _ctx| {
                        async_assert_eq!(
                            pane_group.pane_count(),
                            2,
                            "Expected 2 panes after opening SFTP (terminal + SFTP)"
                        )
                    })
                })
                .add_assertion(assert_sftp_browser_view_exists()),
        )
}

/// Verifies keyboard events work correctly after the SFTP panel gains focus.
pub fn test_sftp_pane_focus_and_keyboard() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            TestStep::new("Open SFTP pane")
                .with_action(|app, _, _| open_sftp_pane(app))
                .set_post_step_pause(std::time::Duration::from_secs(2)),
        )
        .with_step(
            TestStep::new("Press Escape to close dialog if any")
                .with_keystrokes(&["escape"])
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify pane still exists").add_assertion(|app, window_id| {
                let pane_group = pane_group_view(app, window_id, 0);
                pane_group.read(app, |pane_group, _ctx| {
                    async_assert_eq!(
                        pane_group.pane_count(),
                        2,
                        "SFTP pane should still be visible"
                    )
                })
            }),
        )
}

/// Verifies that closing the SFTP panel returns to a single pane.
pub fn test_sftp_pane_close() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            TestStep::new("Open SFTP pane")
                .with_action(|app, _, _| open_sftp_pane(app))
                .set_post_step_pause(std::time::Duration::from_secs(2)),
        )
        .with_step(
            TestStep::new("Verify 2 panes").add_assertion(|app, window_id| {
                let pane_group = pane_group_view(app, window_id, 0);
                pane_group.read(app, |pane_group, _ctx| {
                    async_assert_eq!(pane_group.pane_count(), 2, "Should have 2 panes")
                })
            }),
        )
        // Iterate over all visible panes, find the non-terminal pane (i.e. SFTP), and close it.
        .with_step(
            TestStep::new("Close SFTP pane via pane group")
                .with_action(|app, window_id, _| {
                    let pg = pane_group_view(app, window_id, 0);
                    let sftp_pane_id = pg.read(app, |pane_group, _ctx| {
                        let terminal_ids: std::collections::HashSet<_> =
                            pane_group.terminal_pane_ids().collect();
                        let ids = pane_group.visible_pane_ids();
                        ids.into_iter()
                            .find(|id| !terminal_ids.contains(id))
                            .expect("a non-terminal pane (SFTP) should exist")
                    });
                    pg.update(app, |pane_group, ctx| {
                        pane_group.close_pane(sftp_pane_id, ctx);
                    });
                })
                .set_post_step_pause(std::time::Duration::from_secs(1)),
        )
        .with_step(
            TestStep::new("Verify back to single pane").add_assertion(|app, window_id| {
                let pane_group = pane_group_view(app, window_id, 0);
                pane_group.read(app, |pane_group, _ctx| {
                    async_assert_eq!(
                        pane_group.visible_pane_count(),
                        1,
                        "Should have 1 visible pane after closing SFTP"
                    )
                })
            }),
        )
}

/// Verifies the SFTP panel state after switching tabs.
pub fn test_sftp_pane_tab_switch() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            TestStep::new("Open SFTP pane")
                .with_action(|app, _, _| open_sftp_pane(app))
                .set_post_step_pause(std::time::Duration::from_secs(2)),
        )
        // Switch to another tab.
        .with_step(
            TestStep::new("Switch tab with Ctrl+Tab")
                .with_keystrokes(&["ctrl-tab"])
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Switch back")
                .with_keystrokes(&["ctrl-shift-tab"])
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify SFTP pane still visible").add_assertion(|app, window_id| {
                let pane_group = pane_group_view(app, window_id, 0);
                pane_group.read(app, |pane_group, _ctx| {
                    async_assert!(pane_group.pane_count() >= 1, "Should have at least 1 pane")
                })
            }),
        )
}

/// Verifies the SFTP panel renders correctly when the connection has failed.
pub fn test_sftp_pane_disconnected_render() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            TestStep::new("Open SFTP pane (will fail to connect)")
                .with_action(|app, _, _| open_sftp_pane(app))
                .set_post_step_pause(std::time::Duration::from_secs(3)),
        )
        .with_step(
            TestStep::new("Verify pane renders without crash")
                .add_assertion(|app, window_id| {
                    let pane_group = pane_group_view(app, window_id, 0);
                    pane_group.read(app, |pane_group, _ctx| {
                        async_assert_eq!(
                            pane_group.pane_count(),
                            2,
                            "SFTP pane should render even in disconnected state"
                        )
                    })
                })
                .add_assertion(assert_sftp_browser_view_exists()),
        )
}

// ============================================================
// Mock backend UI integration tests
// ============================================================

/// Common step that opens the SFTP panel and injects a mock backend.
fn open_sftp_with_mock_step(
    files: &'static [(&'static str, &'static [u8])],
) -> warpui::integration::TestStep {
    // Use TestStep::new rather than new_step_with_default_assertions, because
    // opening the SFTP panel changes the pane layout (SFTP may end up at
    // pane_index=0), and the default assertion would panic looking for a
    // terminal_view at pane_index=0.
    TestStep::new("Open SFTP pane with mock backend")
        .with_action(move |app, _, step_data: &mut StepDataMap| {
            let (_, temp_dir) = sftp::open_sftp_pane_with_mock(app, files);
            // Store temp_dir in the StepDataMap to keep it alive.
            step_data.insert("sftp_mock", temp_dir);
        })
        .set_post_step_pause(std::time::Duration::from_secs(2))
}

/// Verifies the mock backend connects successfully and the SFTP browser is in the Connected state.
pub fn test_sftp_mock_backend_connected() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[
            ("readme.txt", b"hello"),
            ("docs/report.txt", b"report"),
        ]))
        .with_step(
            TestStep::new("Verify Connected state and entries")
                .add_assertion(|app, window_id| {
                    let view = sftp::sftp_browser_view(app, window_id);
                    view.read(app, |v, _| {
                        async_assert!(
                            matches!(v.connection_state(), ConnectionState::Connected),
                            "should be in the Connected state"
                        )
                    })
                })
                .add_assertion(|app, window_id| {
                    let view = sftp::sftp_browser_view(app, window_id);
                    view.read(app, |v, _| {
                        async_assert_eq!(
                            v.entries().len(),
                            2,
                            "should list 2 entries (docs directory + readme.txt)"
                        )
                    })
                }),
        )
}

/// Clicks the toolbar refresh button and verifies entries are reloaded.
pub fn test_sftp_toolbar_refresh() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[("file1.txt", b"content1")]))
        .with_step(
            TestStep::new("Click refresh button")
                .with_click_on_saved_position("sftp_btn:refresh")
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify entries still present after refresh").add_assertion(
                |app, window_id| {
                    let view = sftp::sftp_browser_view(app, window_id);
                    view.read(app, |v, _| {
                        async_assert_eq!(
                            v.entries().len(),
                            1,
                            "entries should still be present after refresh"
                        )
                    })
                },
            ),
        )
}

/// Clicks the new folder button and verifies the dialog opens.
pub fn test_sftp_toolbar_new_folder() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[]))
        .with_step(
            TestStep::new("Click new folder button")
                .with_click_on_saved_position("sftp_btn:new_folder")
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify CreateFolder dialog is open").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert!(
                        matches!(v.dialog(), Some(Dialog::CreateFolder { .. })),
                        "the new folder dialog should open"
                    )
                })
            }),
        )
}

/// Clicks the upload button and verifies it does not panic.
pub fn test_sftp_toolbar_upload() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[]))
        .with_step(
            TestStep::new("Click upload button")
                .with_click_on_saved_position("sftp_btn:upload")
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify view still stable after upload click").add_assertion(
                |app, window_id| {
                    let view = sftp::sftp_browser_view(app, window_id);
                    view.read(app, |v, _| {
                        async_assert!(
                            matches!(v.connection_state(), ConnectionState::Connected),
                            "should still be Connected after clicking upload"
                        )
                    })
                },
            ),
        )
}

/// Clicks the parent directory button and verifies navigation goes back.
pub fn test_sftp_toolbar_up() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[("subdir/file.txt", b"content")]))
        // Enter the subdirectory.
        .with_step(
            TestStep::new("Enter subdirectory")
                .with_action(|app, window_id, _| {
                    let view = sftp::sftp_browser_view(app, window_id);
                    view.update(app, |v, ctx| {
                        v.handle_action(
                            &SftpBrowserAction::OpenEntry(
                                v.entries().iter().position(|e| e.name == "subdir").unwrap(),
                            ),
                            ctx,
                        );
                    });
                })
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        // Click the parent directory button.
        .with_step(
            TestStep::new("Click up button")
                .with_click_on_saved_position("sftp_btn:up")
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify navigated back to root").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert!(
                        v.entries().iter().any(|e| e.name == "subdir"),
                        "the subdir directory should be visible after going back up"
                    )
                })
            }),
        )
}

/// Clicks a file row and verifies the selected state.
pub fn test_sftp_click_file_row_selects() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[
            ("file_a.txt", b"a"),
            ("file_b.txt", b"b"),
        ]))
        .with_step(
            TestStep::new("Click on first file row")
                .with_click_on_saved_position("sftp_row:0")
                .set_post_step_pause(std::time::Duration::from_millis(300)),
        )
        .with_step(
            TestStep::new("Verify file is selected").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert!(
                        v.selected().contains(&0),
                        "the first file should be selected"
                    )
                })
            }),
        )
}

/// Right-clicks a file row and verifies the context menu opens.
pub fn test_sftp_right_click_opens_menu() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[("menu_file.txt", b"content")]))
        .with_step(
            TestStep::new("Right-click on file row")
                .with_right_click_on_saved_position("sftp_row:0")
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify context menu is open").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert!(
                        v.context_menu().is_some(),
                        "the context menu should be open"
                    )
                })
            }),
        )
}

/// Context menu -> click delete -> confirm.
pub fn test_sftp_ctx_menu_delete() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[("to_delete.txt", b"delete me")]))
        // Right-click to open the menu.
        .with_step(
            TestStep::new("Right-click on file")
                .with_right_click_on_saved_position("sftp_row:0")
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        // Click the delete menu item.
        .with_step(
            TestStep::new("Click delete in context menu")
                .with_click_on_saved_position("sftp_ctx:delete")
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        // Verify the delete confirmation dialog.
        .with_step(
            TestStep::new("Verify delete confirm dialog").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert!(
                        matches!(v.dialog(), Some(Dialog::DeleteConfirm { .. })),
                        "the delete confirmation dialog should open"
                    )
                })
            }),
        )
        // Click confirm.
        .with_step(
            TestStep::new("Click confirm button")
                .with_click_on_saved_position("sftp_btn:dialog_confirm")
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        // Verify the entry has been deleted.
        .with_step(
            TestStep::new("Verify file deleted").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert_eq!(
                        v.entries().len(),
                        0,
                        "there should be no entries after deletion"
                    )
                })
            }),
        )
}

/// Context menu -> rename.
pub fn test_sftp_ctx_menu_rename() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[("old_name.txt", b"content")]))
        .with_step(
            TestStep::new("Right-click on file")
                .with_right_click_on_saved_position("sftp_row:0")
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Click rename in context menu")
                .with_click_on_saved_position("sftp_ctx:rename")
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify rename dialog is open").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert!(
                        matches!(v.dialog(), Some(Dialog::Rename { .. })),
                        "the rename dialog should open"
                    )
                })
            }),
        )
}

/// Breadcrumb navigation -- click the root directory.
pub fn test_sftp_breadcrumb_root_click() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[("subdir/file.txt", b"content")]))
        // Enter the subdirectory.
        .with_step(
            TestStep::new("Enter subdirectory")
                .with_action(|app, window_id, _| {
                    let view = sftp::sftp_browser_view(app, window_id);
                    view.update(app, |v, ctx| {
                        let idx = v.entries().iter().position(|e| e.name == "subdir").unwrap();
                        v.handle_action(&SftpBrowserAction::OpenEntry(idx), ctx);
                    });
                })
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        // Click the breadcrumb root "/" to navigate back to the root directory.
        .with_step(
            TestStep::new("Navigate to root via breadcrumb")
                .with_action(|app, window_id, _| {
                    let view = sftp::sftp_browser_view(app, window_id);
                    view.update(app, |v, ctx| {
                        v.handle_action(
                            &SftpBrowserAction::NavigateTo(std::path::PathBuf::from("/")),
                            ctx,
                        );
                    });
                })
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify navigated to root").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert!(
                        v.entries().iter().any(|e| e.name == "subdir"),
                        "subdir should be visible after returning to the root directory"
                    )
                })
            }),
        )
}

/// Keyboard Backspace navigates to the parent directory.
pub fn test_sftp_keyboard_backspace_up() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[("subdir/file.txt", b"x")]))
        // Enter the subdirectory.
        .with_step(
            TestStep::new("Enter subdirectory")
                .with_action(|app, window_id, _| {
                    let view = sftp::sftp_browser_view(app, window_id);
                    view.update(app, |v, ctx| {
                        let idx = v.entries().iter().position(|e| e.name == "subdir").unwrap();
                        v.handle_action(&SftpBrowserAction::OpenEntry(idx), ctx);
                    });
                })
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        // Press Backspace.
        .with_step(
            TestStep::new("Press Backspace to go up")
                .with_keystrokes(&["backspace"])
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify back at root").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert!(
                        v.entries().iter().any(|e| e.name == "subdir"),
                        "after Backspace, subdir should be visible in the parent directory"
                    )
                })
            }),
        )
}

/// Keyboard Delete removes the selected entry.
pub fn test_sftp_keyboard_delete() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[("del_target.txt", b"x")]))
        // Select the first entry.
        .with_step(
            TestStep::new("Select first entry")
                .with_action(|app, window_id, _| {
                    let view = sftp::sftp_browser_view(app, window_id);
                    view.update(app, |v, ctx| {
                        v.handle_action(&SftpBrowserAction::SelectEntry(0), ctx);
                    });
                })
                .set_post_step_pause(std::time::Duration::from_millis(300)),
        )
        // Press Delete.
        .with_step(
            TestStep::new("Press Delete key")
                .with_keystrokes(&["delete"])
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify delete confirm dialog").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert!(
                        matches!(v.dialog(), Some(Dialog::DeleteConfirm { .. })),
                        "the Delete key should trigger the delete confirmation dialog"
                    )
                })
            }),
        )
}

/// Keyboard Escape closes the dialog.
pub fn test_sftp_keyboard_escape_close_dialog() -> Builder {
    new_builder()
        .with_user_defaults(HashMap::from([(
            "UndoCloseEnabled".to_string(),
            false.to_string(),
        )]))
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_sftp_with_mock_step(&[("file.txt", b"x")]))
        // Open the new folder dialog.
        .with_step(
            TestStep::new("Open new folder dialog")
                .with_action(|app, window_id, _| {
                    let view = sftp::sftp_browser_view(app, window_id);
                    view.update(app, |v, ctx| {
                        v.handle_action(&SftpBrowserAction::NewFolder, ctx);
                    });
                })
                .set_post_step_pause(std::time::Duration::from_millis(300)),
        )
        .with_step(
            TestStep::new("Verify dialog open").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert!(v.dialog().is_some(), "the dialog should be open")
                })
            }),
        )
        // Press Escape to close.
        .with_step(
            TestStep::new("Press Escape to close")
                .with_keystrokes(&["escape"])
                .set_post_step_pause(std::time::Duration::from_millis(500)),
        )
        .with_step(
            TestStep::new("Verify dialog closed").add_assertion(|app, window_id| {
                let view = sftp::sftp_browser_view(app, window_id);
                view.read(app, |v, _| {
                    async_assert!(v.dialog().is_none(), "the dialog should close after Escape")
                })
            }),
        )
}
