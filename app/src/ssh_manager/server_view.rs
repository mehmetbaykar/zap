//! BackingView implementation for the central pane of the SSH server editor.
//!
//! Phase 2: editable form (name / host / port / user / auth / password / key_path) +
//! a "Save" button in the top-right corner + Auth type toggle (password / private key).
//!
//! From Phase 3, add a "Connect" button → emit OpenSshTerminal → SecretInjector.

use crate::editor::{
    EditorView, Event as EditorEvent, SingleLineEditorOptions, TextColors, TextOptions,
};
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::pane::view;
use crate::pane_group::{BackingView, PaneConfiguration, PaneEvent};
use crate::ssh_manager::{SshTreeChangedEvent, SshTreeChangedNotifier};
use crate::view_components::dropdown::{Dropdown, DropdownItem};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    Align, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CornerRadius, CrossAxisAlignment, Element, Fill, Flex, Hoverable, MainAxisAlignment,
    MainAxisSize, MouseStateHandle, ParentElement, Radius, ScrollbarWidth, Shrinkable, Text,
};
use warpui::fonts::Weight;
use warpui::platform::{Cursor, FilePickerConfiguration};
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use warp_ssh_manager::{
    AuthType, ConnectionStatus, KeychainSecretStore, NodeKind, SecretKind, SshNode, SshRepository,
    SshSecretStore, SshSecretStoreError, SshServerInfo,
};
use zeroize::Zeroizing;

const FIELD_LABEL_MARGIN_TOP: f32 = 6.0;
const FIELD_LABEL_MARGIN_BOTTOM: f32 = 4.0;
const FIELD_BLOCK_MARGIN_BOTTOM: f32 = 12.0;
const SAVE_BUTTON_WIDTH: f32 = 96.0;
const SAVE_BUTTON_HEIGHT: f32 = 28.0;
const AUTH_TOGGLE_PADDING_H: f32 = 14.0;
const AUTH_TOGGLE_PADDING_V: f32 = 6.0;

#[derive(Debug, Clone, Copy)]
pub enum SshServerAction {
    Save,
    Connect,
    TestConnection,
    SetAuthPassword,
    SetAuthKey,
    /// Open the system file picker to choose a private key file, and write the path into the key_path editor.
    PickKeyFile,
    /// Select a group (None means root level, Some(index) means self.folders[index]).
    SelectGroup(Option<usize>),
}

/// A status label shown above/below the Save button for a single occurrence.
#[derive(Debug, Clone)]
enum StatusBanner {
    Saved,
    Success(String),
    Error(String),
}

pub struct SshServerView {
    node_id: String,
    /// Node metadata (mainly uses name as the header title).
    node: Option<SshNode>,
    /// Caches the server last read from the DB, used for placeholder text and initial values. Folder nodes are None.
    server: Option<SshServerInfo>,
    pane_configuration: ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,

    name_editor: ViewHandle<EditorView>,
    host_editor: ViewHandle<EditorView>,
    port_editor: ViewHandle<EditorView>,
    user_editor: ViewHandle<EditorView>,
    password_editor: ViewHandle<EditorView>,
    key_path_editor: ViewHandle<EditorView>,
    root_password_editor: ViewHandle<EditorView>,
    startup_command_editor: ViewHandle<EditorView>,
    notes_editor: ViewHandle<EditorView>,

    /// The currently selected authentication method. The Save button commits this value to the DB.
    auth_type: AuthType,

    save_btn_state: MouseStateHandle,
    connect_btn_state: MouseStateHandle,
    test_btn_state: MouseStateHandle,
    auth_password_btn_state: MouseStateHandle,
    auth_key_btn_state: MouseStateHandle,
    key_path_picker_btn_state: MouseStateHandle,

    /// The group dropdown selector.
    group_dropdown: ViewHandle<Dropdown<SshServerAction>>,
    /// Caches all folder nodes (id, name), used to rebuild the dropdown list.
    folders: Vec<(String, String)>,
    /// The currently selected group ID (None means root level).
    current_group_id: Option<String>,
    /// The parent_id initially read from the DB, used to decide whether move_node is needed on save.
    original_parent_id: Option<String>,

    status: Option<StatusBanner>,
    connection_status: ConnectionStatus,
    latency_ms: Option<u64>,
    is_testing: bool,
    scroll_state: ClippedScrollStateHandle,
}

impl SshServerView {
    pub fn new(node_id: String, ctx: &mut ViewContext<Self>) -> Self {
        let name_editor = make_editor(false, &crate::t!("common-name"), ctx);
        let host_editor = make_editor(false, "example.com", ctx);
        let port_editor = make_editor(false, "22", ctx);
        let user_editor = make_editor(false, "root", ctx);
        let password_editor = make_editor(true, "•••••••", ctx);
        let key_path_editor = make_editor(false, "/home/user/.ssh/id_ed25519", ctx);
        let root_password_editor = make_editor(
            true,
            &crate::t!("workspace-left-panel-ssh-manager-root-password-placeholder"),
            ctx,
        );
        let startup_command_editor = make_editor(
            false,
            &crate::t!("workspace-left-panel-ssh-manager-startup-command-placeholder"),
            ctx,
        );
        let notes_editor = make_editor(
            false,
            &crate::t!("workspace-left-panel-ssh-manager-notes-placeholder"),
            ctx,
        );

        let pane_configuration = ctx.add_model(|_ctx| PaneConfiguration::new("SSH server"));

        let group_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dd = Dropdown::new(ctx);
            dd.set_main_axis_size(MainAxisSize::Max, ctx);
            dd
        });

        let mut me = Self {
            node_id,
            node: None,
            server: None,
            pane_configuration,
            focus_handle: None,
            name_editor,
            host_editor,
            port_editor,
            user_editor,
            password_editor,
            key_path_editor,
            root_password_editor,
            startup_command_editor,
            notes_editor,
            auth_type: AuthType::Password,
            save_btn_state: MouseStateHandle::default(),
            connect_btn_state: MouseStateHandle::default(),
            test_btn_state: MouseStateHandle::default(),
            auth_password_btn_state: MouseStateHandle::default(),
            auth_key_btn_state: MouseStateHandle::default(),
            key_path_picker_btn_state: MouseStateHandle::default(),
            group_dropdown,
            folders: Vec::new(),
            current_group_id: None,
            original_parent_id: None,
            status: None,
            connection_status: ConnectionStatus::Unknown,
            latency_ms: None,
            is_testing: false,
            scroll_state: ClippedScrollStateHandle::default(),
        };
        me.reload(ctx);

        // Listen to each editor: editing → clear the status banner; ClearParentSelections →
        // clear all other editors' selections (otherwise multiple inputs would be highlighted at once when switching fields).
        let editors = [
            me.name_editor.clone(),
            me.host_editor.clone(),
            me.port_editor.clone(),
            me.user_editor.clone(),
            me.password_editor.clone(),
            me.key_path_editor.clone(),
            me.root_password_editor.clone(),
            me.startup_command_editor.clone(),
            me.notes_editor.clone(),
        ];
        for editor in editors {
            ctx.subscribe_to_view(&editor, |me, source, event, ctx| match event {
                EditorEvent::Edited(_) | EditorEvent::Enter => {
                    if me.status.is_some() {
                        me.status = None;
                        ctx.notify();
                    }
                }
                EditorEvent::Blurred => {
                    // On blur, also clear this editor's own selection, to prevent "after clicking another editor,
                    // the old editor still shows a highlighted select-all".
                    source.update(ctx, |e, ctx| e.clear_selections(ctx));
                    if me.status.is_some() {
                        me.status = None;
                        ctx.notify();
                    }
                }
                EditorEvent::Focused | EditorEvent::ClearParentSelections => {
                    me.clear_other_editors_selections(&source, ctx);
                }
                _ => {}
            });
        }

        me
    }

    fn clear_other_editors_selections(
        &mut self,
        active: &ViewHandle<EditorView>,
        ctx: &mut ViewContext<Self>,
    ) {
        let all = [
            self.name_editor.clone(),
            self.host_editor.clone(),
            self.port_editor.clone(),
            self.user_editor.clone(),
            self.password_editor.clone(),
            self.key_path_editor.clone(),
            self.root_password_editor.clone(),
            self.startup_command_editor.clone(),
            self.notes_editor.clone(),
        ];
        for editor in all {
            if editor != *active {
                editor.update(ctx, |e, ctx| e.clear_selections(ctx));
            }
        }
    }

    pub fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    /// Read node + server from the DB and write the current buffer into each editor.
    fn reload(&mut self, ctx: &mut ViewContext<Self>) {
        let id = self.node_id.clone();
        let result = warp_ssh_manager::with_conn(|c| {
            let nodes = SshRepository::list_nodes(c)?;
            let node = nodes.iter().find(|n| n.id == id).cloned();
            let server = match node.as_ref().map(|n| n.kind) {
                Some(NodeKind::Server) => SshRepository::get_server(c, &id)?,
                _ => None,
            };
            // Collect all folder nodes (id, name)
            let folders: Vec<(String, String)> = nodes
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::Folder))
                .map(|n| (n.id.clone(), n.name.clone()))
                .collect();
            Ok((node, server, folders))
        });
        match result {
            Ok((node, server, folders)) => {
                self.original_parent_id = node.as_ref().and_then(|n| n.parent_id.clone());
                self.current_group_id = self.original_parent_id.clone();
                self.node = node;
                self.server = server;
                self.folders = folders;
            }
            Err(e) => {
                log::error!("ssh_server_view: reload failed: {e:?}");
                self.node = None;
                self.server = None;
                self.folders = Vec::new();
                self.original_parent_id = None;
                self.current_group_id = None;
            }
        }

        // Write the node name / server fields into the editor buffers
        let name = self
            .node
            .as_ref()
            .map(|n| n.name.clone())
            .unwrap_or_default();
        self.name_editor
            .update(ctx, |e, ctx| e.set_buffer_text(&name, ctx));

        if let Some(srv) = self.server.as_ref() {
            self.auth_type = srv.auth_type;
            let host = srv.host.clone();
            let port_str = srv.port.to_string();
            let user = srv.username.clone();
            let key_path = srv.key_path.clone().unwrap_or_default();
            self.host_editor
                .update(ctx, |e, ctx| e.set_buffer_text(&host, ctx));
            self.port_editor
                .update(ctx, |e, ctx| e.set_buffer_text(&port_str, ctx));
            self.user_editor
                .update(ctx, |e, ctx| e.set_buffer_text(&user, ctx));
            self.key_path_editor
                .update(ctx, |e, ctx| e.set_buffer_text(&key_path, ctx));

            // Password: fill once only when the keychain has content, otherwise keep empty (only overwrite when the user enters a new value).
            // Note: don't show the plaintext password; only give an all-• placeholder when it "exists" in the keychain — this does
            // not affect save semantics (empty string keeps the password unchanged; non-empty string overwrites).
            // Here we just clear the buffer, leaving the password in the keychain; on Save we only write when the buffer is non-empty.
            // The placeholder mode mirrors root_password_editor (already in keychain → "●●●●●●●";
            // not stored → back to the "•••••••" set in new()), giving the user a visual hint that "you can Test even if left blank".
            let pw_saved = KeychainSecretStore
                .get(&srv.node_id, SecretKind::Password)
                .unwrap_or(None)
                .is_some();
            self.password_editor.update(ctx, |e, ctx| {
                e.set_buffer_text("", ctx);
                if pw_saved {
                    e.set_placeholder_text("●●●●●●●", ctx);
                } else {
                    e.set_placeholder_text("•••••••", ctx);
                }
            });
            let startup_command = srv.startup_command.clone().unwrap_or_default();
            self.startup_command_editor
                .update(ctx, |e, ctx| e.set_buffer_text(&startup_command, ctx));
            let notes = srv.notes.clone().unwrap_or_default();
            self.notes_editor
                .update(ctx, |e, ctx| e.set_buffer_text(&notes, ctx));
            // Root password: detect whether it's already saved in the keychain, and show a placeholder hint when it is.
            let root_pw_saved = KeychainSecretStore
                .get(&srv.node_id, SecretKind::RootPassword)
                .unwrap_or(None)
                .is_some();
            self.root_password_editor.update(ctx, |e, ctx| {
                e.set_buffer_text("", ctx);
                if root_pw_saved {
                    e.set_placeholder_text("●●●●●●●", ctx);
                } else {
                    e.set_placeholder_text(
                        &crate::t!("workspace-left-panel-ssh-manager-root-password-placeholder"),
                        ctx,
                    );
                }
            });
        }

        // `set_buffer_text` by default puts every editor in a "select-all" state (buffer replace +
        // default selection), so the first render would show multiple inputs highlighted at once. Clear them one by one.
        let editors = [
            self.name_editor.clone(),
            self.host_editor.clone(),
            self.port_editor.clone(),
            self.user_editor.clone(),
            self.password_editor.clone(),
            self.key_path_editor.clone(),
            self.root_password_editor.clone(),
            self.startup_command_editor.clone(),
            self.notes_editor.clone(),
        ];
        for editor in editors {
            editor.update(ctx, |e, ctx| e.clear_selections(ctx));
        }

        self.rebuild_group_dropdown(ctx);
        ctx.notify();
    }

    /// Rebuild the dropdown list from self.folders and set the current selection.
    fn rebuild_group_dropdown(&mut self, ctx: &mut ViewContext<Self>) {
        let root_label = crate::t!("workspace-left-panel-ssh-manager-group-root");
        let mut items: Vec<DropdownItem<SshServerAction>> = vec![DropdownItem::new(
            root_label,
            SshServerAction::SelectGroup(None),
        )];
        for (i, (_, name)) in self.folders.iter().enumerate() {
            items.push(DropdownItem::new(
                name.clone(),
                SshServerAction::SelectGroup(Some(i)),
            ));
        }

        // Find the index corresponding to the current group
        let selected_index = if let Some(ref gid) = self.current_group_id {
            self.folders
                .iter()
                .position(|(id, _)| id == gid)
                .map(|pos| pos + 1) // +1 because index 0 is "Root"
                .unwrap_or_else(|| {
                    // The folder was deleted externally; fall back to root level
                    self.current_group_id = None;
                    0
                })
        } else {
            0 // Root
        };

        self.group_dropdown.update(ctx, |dd, ctx| {
            dd.set_items(items, ctx);
            dd.set_selected_by_index(selected_index, ctx);
        });
    }

    fn current_text(&self, editor: &ViewHandle<EditorView>, app: &AppContext) -> String {
        editor.as_ref(app).buffer_text(app)
    }

    /// Get the currently selected group ID.
    pub fn current_group_id(&self) -> &Option<String> {
        &self.current_group_id
    }

    /// Get a reference to all folders (id, name) (used for testing).
    pub fn folders(&self) -> &[(String, String)] {
        &self.folders
    }

    fn on_save(&mut self, ctx: &mut ViewContext<Self>) {
        // 1. Collect fields
        let name = self.current_text(&self.name_editor.clone(), ctx);
        let host = self.current_text(&self.host_editor.clone(), ctx);
        let port_str = self.current_text(&self.port_editor.clone(), ctx);
        let user = self.current_text(&self.user_editor.clone(), ctx);
        let password = self.current_text(&self.password_editor.clone(), ctx);
        let key_path_text = self.current_text(&self.key_path_editor.clone(), ctx);
        let root_password = self.current_text(&self.root_password_editor.clone(), ctx);
        let startup_command_text = self.current_text(&self.startup_command_editor.clone(), ctx);
        let notes_text = self.current_text(&self.notes_editor.clone(), ctx);

        let name = name.trim().to_string();
        if name.is_empty() {
            self.status = Some(StatusBanner::Error(crate::t!(
                "workspace-left-panel-ssh-manager-error-name-required"
            )));
            ctx.notify();
            return;
        }

        let port: u16 = match port_str.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                self.status = Some(StatusBanner::Error(crate::t!(
                    "workspace-left-panel-ssh-manager-error-port-invalid"
                )));
                ctx.notify();
                return;
            }
        };

        let key_path = key_path_text.trim().to_string();
        let info = SshServerInfo {
            node_id: self.node_id.clone(),
            host: host.trim().to_string(),
            port,
            username: user.trim().to_string(),
            auth_type: self.auth_type,
            key_path: if key_path.is_empty() {
                None
            } else {
                Some(key_path)
            },
            startup_command: if startup_command_text.trim().is_empty() {
                None
            } else {
                Some(startup_command_text.trim().to_string())
            },
            notes: if notes_text.trim().is_empty() {
                None
            } else {
                Some(notes_text.trim().to_string())
            },
            last_connected_at: self.server.as_ref().and_then(|s| s.last_connected_at),
        };

        // 2. Write the DB (rename + update_server + possibly move_node)
        let id = self.node_id.clone();
        let info_for_db = info.clone();
        let name_for_db = name.clone();
        let group_changed = self.current_group_id != self.original_parent_id;
        let new_parent_id = self.current_group_id.clone();
        let result = warp_ssh_manager::with_conn(move |c| {
            SshRepository::rename_node(c, &id, &name_for_db)?;
            SshRepository::update_server(c, &info_for_db)?;
            if group_changed {
                let new_parent = new_parent_id.as_deref();
                SshRepository::move_node_to_end(c, &id, new_parent)?;
            }
            Ok(())
        });
        if let Err(e) = result {
            log::error!("ssh_server_view: save failed: {e:?}");
            self.status = Some(StatusBanner::Error(format!("{e}")));
            ctx.notify();
            return;
        }

        // 3. Write the keychain (only overwrite when the buffer is non-empty). When auth_type switches to password and the user didn't fill it in,
        //    keep the existing keychain entry; when switching to private key, don't touch the password entry (the user can delete it separately).
        let store = KeychainSecretStore;
        if !password.is_empty() {
            let kind = match self.auth_type {
                AuthType::Password => SecretKind::Password,
                AuthType::Key => SecretKind::Passphrase,
            };
            if let Err(e) = store.set(&self.node_id, kind, &password) {
                log::error!("ssh_server_view: keychain write failed: {e:?}");
                self.status = Some(StatusBanner::Error(format!("keychain: {e}")));
                ctx.notify();
                return;
            }
            // After writing the password field, clear the buffer to avoid plaintext lingering in memory.
            self.password_editor
                .update(ctx, |e, ctx| e.set_buffer_text("", ctx));
        }

        // Root password
        if !root_password.is_empty() {
            if let Err(e) = store.set(&self.node_id, SecretKind::RootPassword, &root_password) {
                log::error!("ssh_server_view: root password keychain write failed: {e:?}");
                self.status = Some(StatusBanner::Error(format!("keychain: {e}")));
                ctx.notify();
                return;
            }
            self.root_password_editor
                .update(ctx, |e, ctx| e.set_buffer_text("", ctx));
        }

        // 4. reload + status hint + notify all SshManagerPanels to refresh the tree
        self.reload(ctx);
        self.status = Some(StatusBanner::Saved);
        SshTreeChangedNotifier::handle(ctx).update(ctx, |_, ctx| {
            ctx.emit(SshTreeChangedEvent::TreeChanged);
        });
        ctx.notify();
    }

    /// Trigger an SSH connection — hand the current node + server config to the Workspace, which opens a new
    /// terminal pane running `ssh ...`. **Prefer the editor's current values** (the user may have changed
    /// fields without Saving), so the connection uses the config "the user sees on screen", not the stale one in the DB.
    fn on_connect(&mut self, ctx: &mut ViewContext<Self>) {
        let host = self.current_text(&self.host_editor.clone(), ctx);
        let port_str = self.current_text(&self.port_editor.clone(), ctx);
        let user = self.current_text(&self.user_editor.clone(), ctx);
        let key_path_text = self.current_text(&self.key_path_editor.clone(), ctx);
        let startup_command_text = self.current_text(&self.startup_command_editor.clone(), ctx);
        let notes_text = self.current_text(&self.notes_editor.clone(), ctx);

        let port: u16 = port_str.trim().parse().unwrap_or(22);
        let host = host.trim().to_string();
        if host.is_empty() {
            self.status = Some(StatusBanner::Error(crate::t!(
                "workspace-left-panel-ssh-manager-error-host-required"
            )));
            ctx.notify();
            return;
        }
        let key_path = key_path_text.trim().to_string();
        let server = SshServerInfo {
            node_id: self.node_id.clone(),
            host,
            port,
            username: user.trim().to_string(),
            auth_type: self.auth_type,
            key_path: if key_path.is_empty() {
                None
            } else {
                Some(key_path)
            },
            startup_command: if startup_command_text.trim().is_empty() {
                None
            } else {
                Some(startup_command_text.trim().to_string())
            },
            notes: if notes_text.trim().is_empty() {
                None
            } else {
                Some(notes_text.trim().to_string())
            },
            last_connected_at: self.server.as_ref().and_then(|s| s.last_connected_at),
        };
        ctx.dispatch_typed_action(&crate::workspace::WorkspaceAction::OpenSshTerminal {
            node_id: self.node_id.clone(),
            server,
        });
    }

    fn on_test_connection(&mut self, ctx: &mut ViewContext<Self>) {
        let host = self.current_text(&self.host_editor.clone(), ctx);
        let port_str = self.current_text(&self.port_editor.clone(), ctx);
        let user = self.current_text(&self.user_editor.clone(), ctx);
        let password = self.current_text(&self.password_editor.clone(), ctx);
        let key_path_text = self.current_text(&self.key_path_editor.clone(), ctx);

        let port: u16 = port_str.trim().parse().unwrap_or(22);
        let host = host.trim().to_string();
        if host.is_empty() {
            self.status = Some(StatusBanner::Error(crate::t!(
                "workspace-left-panel-ssh-manager-error-host-required"
            )));
            ctx.notify();
            return;
        }

        let key_path = key_path_text.trim().to_string();
        let server = SshServerInfo {
            node_id: self.node_id.clone(),
            host,
            port,
            username: user.trim().to_string(),
            auth_type: self.auth_type,
            key_path: if key_path.is_empty() {
                None
            } else {
                Some(key_path)
            },
            startup_command: None,
            notes: None,
            last_connected_at: None,
        };

        // The password is immediately wrapped in Zeroizing to ensure it is zeroed in memory throughout
        // after being taken from the UI text field, until the async test task ends and drops it. Priority: form value > keychain > None.
        // See the `resolve_test_password` comment for details.
        let password = resolve_test_password(&self.node_id, &password, &KeychainSecretStore);

        self.is_testing = true;
        self.status = None;
        ctx.notify();

        let node_id = self.node_id.clone();
        ctx.spawn(
            async move {
                let result =
                    warp_ssh_manager::ssh_command::test_connection(&server, password).await;
                (node_id, result)
            },
            |me, (_node_id, result), ctx| {
                me.is_testing = false;
                me.connection_status = result.status;
                me.latency_ms = result.latency_ms;
                match result.status {
                    ConnectionStatus::Online => {
                        let latency_str = result
                            .latency_ms
                            .map(|ms| format!("{ms}ms"))
                            .unwrap_or_else(|| "N/A".into());
                        let msg = result.error_message.unwrap_or_default();
                        if msg.contains("password auth required") {
                            me.status = Some(StatusBanner::Success(format!(
                                "Server reachable - latency: {latency_str}"
                            )));
                        } else {
                            me.status = Some(StatusBanner::Success(format!(
                                "Online - latency: {latency_str}"
                            )));
                        }
                    }
                    ConnectionStatus::Offline => {
                        me.latency_ms = None;
                        let err = result
                            .error_message
                            .unwrap_or_else(|| "Unknown error".into());
                        me.status = Some(StatusBanner::Error(err));
                    }
                    ConnectionStatus::Unknown => {
                        me.latency_ms = None;
                        me.status = None;
                    }
                }
                ctx.notify();
            },
        );
    }

    /// Open the system file picker to choose a private key file, then write it into the key_path editor. The callback ctx
    /// is ViewContext<Self> (the framework keeps the original view context automatically).
    fn on_pick_key_file(&mut self, ctx: &mut ViewContext<Self>) {
        let editor = self.key_path_editor.clone();
        ctx.open_file_picker(
            move |result, ctx| match result {
                Ok(paths) => {
                    if let Some(path) = paths.into_iter().next() {
                        editor.update(ctx, |e, ctx| e.set_buffer_text(&path, ctx));
                    }
                }
                Err(e) => {
                    log::warn!("ssh: file picker failed: {e}");
                }
            },
            FilePickerConfiguration::new(),
        );
    }

    fn on_set_auth(&mut self, auth: AuthType, ctx: &mut ViewContext<Self>) {
        if self.auth_type != auth {
            self.auth_type = auth;
            // Clear the password buffer when switching auth type — password and passphrase have different semantics.
            self.password_editor
                .update(ctx, |e, ctx| e.set_buffer_text("", ctx));
            self.status = None;
            ctx.notify();
        }
    }

    // ---------- render helpers ---------- //

    fn render_label(&self, text: &str, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        Container::new(
            Text::new_inline(
                text.to_string(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.sub_text_color(theme.background()).into())
            .finish(),
        )
        .with_margin_top(FIELD_LABEL_MARGIN_TOP)
        .with_margin_bottom(FIELD_LABEL_MARGIN_BOTTOM)
        .finish()
    }

    fn render_text_field(
        &self,
        label: &str,
        editor: &ViewHandle<EditorView>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text_input = appearance
            .ui_builder()
            .text_input(editor.clone())
            .with_style(UiComponentStyles {
                padding: Some(Coords {
                    left: 10.,
                    right: 10.,
                    top: 6.,
                    bottom: 6.,
                }),
                background: Some(theme.surface_2().into()),
                border_color: Some(internal_colors::neutral_3(theme).into()),
                border_width: Some(1.0),
                border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.0))),
                ..Default::default()
            })
            .build()
            .finish();

        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(self.render_label(label, appearance))
                .with_child(text_input)
                .finish(),
        )
        .with_margin_bottom(FIELD_BLOCK_MARGIN_BOTTOM)
        .finish()
    }

    /// Private key path field: label + (input box + browse button) on one row.
    fn render_key_path_field(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text_input = appearance
            .ui_builder()
            .text_input(self.key_path_editor.clone())
            .with_style(UiComponentStyles {
                padding: Some(Coords {
                    left: 10.,
                    right: 10.,
                    top: 6.,
                    bottom: 6.,
                }),
                background: Some(theme.surface_2().into()),
                border_color: Some(internal_colors::neutral_3(theme).into()),
                border_width: Some(1.0),
                border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.0))),
                ..Default::default()
            })
            .build()
            .finish();

        let icon_color = theme.sub_text_color(theme.background());
        let icon_el = ConstrainedBox::new(
            crate::ui_components::icons::Icon::Folder
                .to_warpui_icon(icon_color)
                .finish(),
        )
        .with_width(16.0)
        .with_height(16.0)
        .finish();
        let browse_btn = Hoverable::new(self.key_path_picker_btn_state.clone(), move |_| {
            Container::new(
                ConstrainedBox::new(icon_el)
                    .with_width(32.0)
                    .with_height(32.0)
                    .finish(),
            )
            .with_uniform_padding(2.0)
            .with_background(theme.surface_2())
            .with_border(
                warpui::elements::Border::all(1.0)
                    .with_border_color(internal_colors::neutral_3(theme)),
            )
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(SshServerAction::PickKeyFile);
        })
        .finish();

        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(Shrinkable::new(1.0, text_input).finish())
            .with_child(browse_btn)
            .finish();

        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(self.render_label(
                    &crate::t!("workspace-left-panel-ssh-manager-detail-key-path"),
                    appearance,
                ))
                .with_child(row)
                .finish(),
        )
        .with_margin_bottom(FIELD_BLOCK_MARGIN_BOTTOM)
        .finish()
    }

    fn render_auth_toggle(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();

        let make_pill = |label: String,
                         active: bool,
                         state: MouseStateHandle,
                         action: SshServerAction|
         -> Box<dyn Element> {
            let main_color = if active {
                theme.main_text_color(theme.accent())
            } else {
                theme.sub_text_color(theme.background())
            };
            let bg = if active {
                theme.accent()
            } else {
                theme.surface_2()
            };
            let label_el = Text::new_inline(
                label,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(main_color.into())
            .finish();

            Hoverable::new(state, move |_| {
                Container::new(label_el)
                    .with_padding_left(AUTH_TOGGLE_PADDING_H)
                    .with_padding_right(AUTH_TOGGLE_PADDING_H)
                    .with_padding_top(AUTH_TOGGLE_PADDING_V)
                    .with_padding_bottom(AUTH_TOGGLE_PADDING_V)
                    .with_background(bg)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                    .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action))
            .finish()
        };

        let pill_password = make_pill(
            crate::t!("workspace-left-panel-ssh-manager-auth-password"),
            matches!(self.auth_type, AuthType::Password),
            self.auth_password_btn_state.clone(),
            SshServerAction::SetAuthPassword,
        );
        let pill_key = make_pill(
            crate::t!("workspace-left-panel-ssh-manager-auth-key"),
            matches!(self.auth_type, AuthType::Key),
            self.auth_key_btn_state.clone(),
            SshServerAction::SetAuthKey,
        );

        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(self.render_label(
                    &crate::t!("workspace-left-panel-ssh-manager-detail-auth"),
                    appearance,
                ))
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(pill_password)
                        .with_child(pill_key)
                        .with_main_axis_size(MainAxisSize::Min)
                        .finish(),
                )
                .finish(),
        )
        .with_margin_bottom(FIELD_BLOCK_MARGIN_BOTTOM)
        .finish()
    }

    fn render_save_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        appearance
            .ui_builder()
            .button(ButtonVariant::Accent, self.save_btn_state.clone())
            .with_style(UiComponentStyles {
                font_color: Some(
                    appearance
                        .theme()
                        .main_text_color(appearance.theme().accent())
                        .into_solid(),
                ),
                font_weight: Some(Weight::Bold),
                width: Some(SAVE_BUTTON_WIDTH),
                height: Some(SAVE_BUTTON_HEIGHT),
                font_size: Some(13.0),
                ..Default::default()
            })
            .with_centered_text_label(crate::t!("workspace-left-panel-ssh-manager-save"))
            .build()
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(SshServerAction::Save))
            .finish()
    }

    fn render_connect_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        appearance
            .ui_builder()
            .button(ButtonVariant::Secondary, self.connect_btn_state.clone())
            .with_style(UiComponentStyles {
                font_weight: Some(Weight::Bold),
                width: Some(SAVE_BUTTON_WIDTH),
                height: Some(SAVE_BUTTON_HEIGHT),
                font_size: Some(13.0),
                ..Default::default()
            })
            .with_centered_text_label(crate::t!("workspace-left-panel-ssh-manager-connect"))
            .build()
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(SshServerAction::Connect))
            .finish()
    }

    fn render_test_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        let label = if self.is_testing {
            crate::t!("workspace-left-panel-ssh-manager-testing")
        } else {
            crate::t!("workspace-left-panel-ssh-manager-test")
        };
        appearance
            .ui_builder()
            .button(ButtonVariant::Secondary, self.test_btn_state.clone())
            .with_style(UiComponentStyles {
                font_weight: Some(Weight::Bold),
                width: Some(SAVE_BUTTON_WIDTH),
                height: Some(SAVE_BUTTON_HEIGHT),
                font_size: Some(13.0),
                ..Default::default()
            })
            .with_centered_text_label(label)
            .build()
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(SshServerAction::TestConnection))
            .finish()
    }

    fn render_connection_status(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let bg = theme.background();
        let (icon, color, text) = match self.connection_status {
            ConnectionStatus::Online => {
                let latency_str = self
                    .latency_ms
                    .map(|ms| format!(" ({ms}ms)"))
                    .unwrap_or_default();
                (
                    "●",
                    theme.ui_green_color().into(),
                    format!(
                        "{}{latency_str}",
                        crate::t!("workspace-left-panel-ssh-manager-status-online")
                    ),
                )
            }
            ConnectionStatus::Offline => (
                "●",
                theme.ui_error_color().into(),
                crate::t!("workspace-left-panel-ssh-manager-status-offline"),
            ),
            ConnectionStatus::Unknown => (
                "○",
                theme.sub_text_color(bg),
                crate::t!("workspace-left-panel-ssh-manager-status-unknown"),
            ),
        };

        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(
                Text::new_inline(icon, appearance.ui_font_family(), 12.0)
                    .with_color(color.into())
                    .finish(),
            )
            .with_child(
                Text::new_inline(text, appearance.ui_font_family(), appearance.ui_font_size())
                    .with_color(color.into())
                    .finish(),
            )
            .with_main_axis_size(MainAxisSize::Min)
            .finish()
    }

    fn render_status_banner(&self, appearance: &Appearance) -> Option<Box<dyn Element>> {
        let theme = appearance.theme();
        let (text, color) = match self.status.as_ref()? {
            StatusBanner::Saved => (
                crate::t!("workspace-left-panel-ssh-manager-status-saved"),
                theme.ui_green_color(),
            ),
            StatusBanner::Success(msg) => (msg.clone(), theme.ui_green_color()),
            StatusBanner::Error(msg) => (msg.clone(), theme.ui_error_color()),
        };
        Some(
            Container::new(
                Text::new_inline(text, appearance.ui_font_family(), appearance.ui_font_size())
                    .with_color(color)
                    .finish(),
            )
            .with_margin_top(8.0)
            .with_margin_bottom(8.0)
            .finish(),
        )
    }

    /// Group dropdown field: label + dropdown.
    fn render_group_field(&self, appearance: &Appearance) -> Box<dyn Element> {
        let label = self.render_label(
            &crate::t!("workspace-left-panel-ssh-manager-field-group"),
            appearance,
        );
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(label)
                .with_child(ChildView::new(&self.group_dropdown).finish())
                .finish(),
        )
        .with_margin_bottom(FIELD_BLOCK_MARGIN_BOTTOM)
        .finish()
    }
}

fn make_editor(
    is_password: bool,
    placeholder: &str,
    ctx: &mut ViewContext<SshServerView>,
) -> ViewHandle<EditorView> {
    let placeholder = placeholder.to_string();
    ctx.add_typed_action_view(move |ctx| {
        let options = {
            let appearance = Appearance::as_ref(ctx);
            let theme = appearance.theme();
            SingleLineEditorOptions {
                is_password,
                text: TextOptions {
                    font_size_override: Some(appearance.ui_font_size()),
                    font_family_override: Some(appearance.monospace_font_family()),
                    text_colors_override: Some(TextColors {
                        default_color: theme.active_ui_text_color(),
                        disabled_color: theme.disabled_ui_text_color(),
                        hint_color: theme.disabled_ui_text_color(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }
        };
        let mut editor = EditorView::single_line(options, ctx);
        editor.set_placeholder_text(&placeholder, ctx);
        editor
    })
}

impl Entity for SshServerView {
    type Event = PaneEvent;
}

impl TypedActionView for SshServerView {
    type Action = SshServerAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            SshServerAction::Save => self.on_save(ctx),
            SshServerAction::Connect => self.on_connect(ctx),
            SshServerAction::TestConnection => self.on_test_connection(ctx),
            SshServerAction::SetAuthPassword => self.on_set_auth(AuthType::Password, ctx),
            SshServerAction::SetAuthKey => self.on_set_auth(AuthType::Key, ctx),
            SshServerAction::PickKeyFile => self.on_pick_key_file(ctx),
            SshServerAction::SelectGroup(index) => {
                let new_group_id =
                    index.and_then(|i| self.folders.get(i).map(|(id, _)| id.clone()));
                if new_group_id != self.current_group_id {
                    self.current_group_id = new_group_id;
                    ctx.notify();
                }
            }
        }
    }
}

impl View for SshServerView {
    fn ui_name() -> &'static str {
        "SshServerView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);

        // folder node / server not found → simple hint + hide the form
        if !matches!(self.node.as_ref().map(|n| n.kind), Some(NodeKind::Server)) {
            let body_text = match self.node.as_ref().map(|n| n.kind) {
                Some(NodeKind::Folder) => {
                    crate::t!("workspace-left-panel-ssh-manager-pane-folder-body")
                }
                _ => crate::t!("workspace-left-panel-ssh-manager-server-missing"),
            };
            let theme = appearance.theme();
            let body = Text::new_inline(
                body_text,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.sub_text_color(theme.background()).into())
            .finish();
            return Align::new(
                ConstrainedBox::new(Container::new(body).with_uniform_padding(24.0).finish())
                    .with_max_width(560.0)
                    .finish(),
            )
            .top_center()
            .finish();
        }

        // ---- header row: title + Save button on the right + status banner ----
        let title_text = self
            .node
            .as_ref()
            .map(|n| n.name.clone())
            .unwrap_or_default();
        let title = Text::new_inline(
            title_text,
            appearance.ui_font_family(),
            appearance.ui_font_heading_2(),
        )
        .with_color(
            appearance
                .theme()
                .main_text_color(appearance.theme().background())
                .into(),
        )
        .finish();

        // Title on the left / [Test] [Connect] [Save] buttons on the right.
        let buttons = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(self.render_test_button(appearance))
            .with_child(self.render_connect_button(appearance))
            .with_child(self.render_save_button(appearance))
            .with_main_axis_size(MainAxisSize::Min)
            .finish();
        let header = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(title)
            .with_child(buttons)
            .finish();

        let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        col.add_child(Container::new(header).with_margin_bottom(8.0).finish());

        col.add_child(
            Container::new(self.render_connection_status(appearance))
                .with_margin_bottom(8.0)
                .finish(),
        );

        if let Some(banner) = self.render_status_banner(appearance) {
            col.add_child(banner);
        }

        col.add_child(self.render_text_field(
            &crate::t!("workspace-left-panel-ssh-manager-field-name"),
            &self.name_editor,
            appearance,
        ));

        // Group dropdown
        col.add_child(self.render_group_field(appearance));

        col.add_child(self.render_text_field(
            &crate::t!("workspace-left-panel-ssh-manager-detail-host"),
            &self.host_editor,
            appearance,
        ));
        col.add_child(self.render_text_field(
            &crate::t!("workspace-left-panel-ssh-manager-detail-port"),
            &self.port_editor,
            appearance,
        ));
        col.add_child(self.render_text_field(
            &crate::t!("workspace-left-panel-ssh-manager-detail-user"),
            &self.user_editor,
            appearance,
        ));
        col.add_child(self.render_auth_toggle(appearance));

        // Show the password or key_path field based on the current auth_type
        match self.auth_type {
            AuthType::Password => {
                col.add_child(self.render_text_field(
                    &crate::t!("workspace-left-panel-ssh-manager-auth-password"),
                    &self.password_editor,
                    appearance,
                ));
            }
            AuthType::Key => {
                col.add_child(self.render_key_path_field(appearance));
                col.add_child(self.render_text_field(
                    &crate::t!("workspace-left-panel-ssh-manager-passphrase"),
                    &self.password_editor,
                    appearance,
                ));
            }
        }

        // Startup command
        col.add_child(self.render_text_field(
            &crate::t!("workspace-left-panel-ssh-manager-startup-command"),
            &self.startup_command_editor,
            appearance,
        ));
        // Root password
        col.add_child(self.render_text_field(
            &crate::t!("workspace-left-panel-ssh-manager-root-password"),
            &self.root_password_editor,
            appearance,
        ));
        // Notes
        col.add_child(self.render_text_field(
            &crate::t!("workspace-left-panel-ssh-manager-notes"),
            &self.notes_editor,
            appearance,
        ));

        let theme = appearance.theme();
        let inner = ConstrainedBox::new(
            Container::new(col.finish())
                .with_uniform_padding(24.0)
                .finish(),
        )
        .with_max_width(640.0)
        .finish();

        // Wrap a layer of ClippedScrollable so content scrolls vertically when it overflows, avoiding overlap with the pane below.
        let scrollbar_color = theme.disabled_text_color(theme.background()).into();
        let scrollbar_thumb_hover = theme.main_text_color(theme.background()).into();
        let scrollable = ClippedScrollable::vertical(
            self.scroll_state.clone(),
            inner,
            ScrollbarWidth::Auto,
            scrollbar_color,
            scrollbar_thumb_hover,
            Fill::None,
        )
        .finish();

        Align::new(scrollable).top_center().finish()
    }
}

impl BackingView for SshServerView {
    type PaneHeaderOverflowMenuAction = SshServerAction;
    type CustomAction = ();
    type AssociatedData = ();

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        action: &Self::PaneHeaderOverflowMenuAction,
        ctx: &mut ViewContext<Self>,
    ) {
        self.handle_action(action, ctx);
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(PaneEvent::Close);
    }

    fn focus_contents(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus(&self.host_editor);
    }

    fn render_header_content(
        &self,
        _ctx: &view::HeaderRenderContext<'_>,
        _app: &AppContext,
    ) -> view::HeaderContent {
        let title = self
            .node
            .as_ref()
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "SSH server".to_string());
        view::HeaderContent::simple(title)
    }

    fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, _ctx: &mut ViewContext<Self>) {
        self.focus_handle = Some(focus_handle);
    }
}

/// Resolve the password source for "test connection", with a fixed priority:
/// 1. form text non-empty → use the form value (the user has typed it, **no
///    requirement** to Save first)
/// 2. form empty + keychain has it → use the value stored in the keychain
/// 3. form empty + keychain missing/error → `None`, and the backend returns "Password not provided"
///
/// The form always wins over the keychain — when the user wants to test after
/// changing host/port, the password being typed is for the new host and should
/// not be overwritten by the stale keychain value.
///
/// author: logic
/// date: 2026-06-01
fn resolve_test_password(
    node_id: &str,
    editor_text: &str,
    store: &dyn SshSecretStore,
) -> Option<Zeroizing<String>> {
    if !editor_text.is_empty() {
        return Some(Zeroizing::new(editor_text.to_string()));
    }
    match store.get(node_id, SecretKind::Password) {
        Ok(Some(secret)) => Some(secret),
        Ok(None) => None,
        Err(SshSecretStoreError::NoBackend) => None,
        Err(SshSecretStoreError::Keyring(msg)) => {
            log::warn!("keychain read failed, fallback failed: {msg}");
            None
        }
    }
}

#[cfg(test)]
#[path = "server_view_tests.rs"]
mod tests;
