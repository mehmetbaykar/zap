//! Language servers and project setup, shown under the Code umbrella.
//!
//! Upstream's page of the same name is built around hosted codebase indexing
//! (`ai::index::full_source_code_embedding`, the remote codebase-index service
//! and the team-admin policy UI). None of that exists in this fork, so what
//! survives here is the local language-server management the fork already had
//! on its combined Code page.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ai::project_context::model::{ProjectContextModel, ProjectContextModelEvent};
use lsp::supported_servers::LSPServerType;
use lsp::{LspManagerModel, LspManagerModelEvent, LspServerModel, LspState};
use warp_core::features::FeatureFlag;
use warp_errors::report_if_error;
use warpui::elements::{
    Container, CornerRadius, CrossAxisAlignment, Element, Flex, MainAxisAlignment, MainAxisSize,
    MouseStateHandle, ParentElement, Radius, Shrinkable,
};
use warpui::fonts::Weight;
use warpui::keymap::ContextPredicate;
use warpui::platform::Cursor;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::ui_components::switch::SwitchStateHandle;
use warpui::{
    Action, AppContext, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use super::settings_page::{
    HEADER_PADDING, MatchData, PageType, SettingsPageMeta, SettingsPageViewHandle, SettingsWidget,
    build_sub_header, render_separator,
};
use super::{SettingsAction, SettingsSection};
use crate::appearance::Appearance;

pub struct CodeIndexingPageView {
    page: PageType<Self>,
}

impl CodeIndexingPageView {
    pub fn new(ctx: &mut ViewContext<CodeIndexingPageView>) -> Self {
        // Subscribe to ProjectContextModel: re-render when the project rules change,
        // so any subcomponent that depends on the rule set stays up to date.
        ctx.subscribe_to_model(&ProjectContextModel::handle(ctx), |_me, _, event, ctx| {
            if matches!(event, ProjectContextModelEvent::KnownRulesChanged(_)) {
                ctx.notify();
            }
        });

        ctx.subscribe_to_model(
            &LspManagerModel::handle(ctx),
            |_me, _, event, ctx| match event {
                LspManagerModelEvent::ServerStarted(_)
                | LspManagerModelEvent::ServerStopped(_)
                | LspManagerModelEvent::ServerRemoved { .. } => ctx.notify(),
            },
        );

        Self {
            page: Self::build_page(),
        }
    }

    fn build_page() -> PageType<Self> {
        let widgets: Vec<Box<dyn SettingsWidget<View = Self>>> =
            if FeatureFlag::ZapNewSettingsModes.is_enabled() {
                vec![Box::new(LspManagementWidget::default())]
            } else {
                // Legacy view: under the old settings mode this page renders nothing.
                vec![]
            };
        PageType::new_uncategorized(widgets, None)
    }
}

impl Entity for CodeIndexingPageView {
    type Event = CodeIndexingPageEvent;
}

impl View for CodeIndexingPageView {
    fn ui_name() -> &'static str {
        "CodeIndexingPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

#[derive(Debug, Clone)]
pub enum CodeIndexingPageEvent {
    OpenProjectRules { rule_paths: Vec<PathBuf> },
}

#[derive(Debug, Clone)]
pub enum CodeIndexingPageAction {
    OpenProjectRules {
        rule_paths: Vec<PathBuf>,
    },
    SetLspServerEnabled {
        server: ModelHandle<LspServerModel>,
        enabled: bool,
    },
    RestartLspServer {
        server: ModelHandle<LspServerModel>,
    },
    OpenLspLogs {
        log_path: PathBuf,
    },
}

impl TypedActionView for CodeIndexingPageView {
    type Action = CodeIndexingPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CodeIndexingPageAction::OpenProjectRules { rule_paths } => {
                ctx.emit(CodeIndexingPageEvent::OpenProjectRules {
                    rule_paths: rule_paths.clone(),
                });
            }
            CodeIndexingPageAction::SetLspServerEnabled { server, enabled } => {
                server.update(ctx, |server, ctx| {
                    if *enabled {
                        report_if_error!(server.manual_start(ctx));
                    } else {
                        report_if_error!(server.stop(true, ctx));
                    }
                });
                ctx.notify();
            }
            CodeIndexingPageAction::RestartLspServer { server } => {
                server.update(ctx, |server, ctx| {
                    server.restart(ctx);
                });
                ctx.notify();
            }
            CodeIndexingPageAction::OpenLspLogs { log_path } => {
                ctx.open_file_path(log_path);
            }
        }
    }
}

impl SettingsPageMeta for CodeIndexingPageView {
    fn section() -> SettingsSection {
        SettingsSection::CodeIndexing
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        FeatureFlag::ZapNewSettingsModes.is_enabled()
    }

    fn on_page_selected(&mut self, _: bool, _ctx: &mut ViewContext<Self>) {}

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<CodeIndexingPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<CodeIndexingPageView>) -> Self {
        SettingsPageViewHandle::CodeIndexing(view_handle)
    }
}

pub fn init_actions_from_parent_view<T: Action + Clone>(
    _app: &mut AppContext,
    _context: &ContextPredicate,
    _builder: fn(SettingsAction) -> T,
) {
}

#[derive(Clone, Default)]
struct LspServerRowMouseStates {
    restart: MouseStateHandle,
    #[cfg(not(target_family = "wasm"))]
    view_logs: MouseStateHandle,
    toggle: SwitchStateHandle,
}

#[derive(Default)]
struct LspManagementWidget {
    row_mouse_states: RefCell<HashMap<(PathBuf, LSPServerType), LspServerRowMouseStates>>,
}

impl LspManagementWidget {
    fn render_server_row(
        &self,
        workspace_path: &Path,
        server_handle: &ModelHandle<LspServerModel>,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let server = server_handle.as_ref(app);
        let server_type = server.server_type();
        let server_state = server.state();
        let is_enabled = server_state.can_auto_start();
        let can_restart = !matches!(server_state, LspState::Starting | LspState::Stopping { .. });
        let (status, status_color) = match server_state {
            LspState::Available { .. } if server.has_pending_tasks() => {
                ("Busy", appearance.theme().ansi_fg_yellow())
            }
            LspState::Available { .. } => ("Available", appearance.theme().ansi_fg_green()),
            LspState::Starting => ("Starting", appearance.theme().ansi_fg_yellow()),
            LspState::Stopping { .. } => ("Stopping", appearance.theme().ansi_fg_yellow()),
            LspState::Stopped {
                manually_stopped: true,
            } => (
                "Disabled",
                appearance.theme().disabled_ui_text_color().into_solid(),
            ),
            LspState::Stopped {
                manually_stopped: false,
            } => (
                "Stopped",
                appearance.theme().disabled_ui_text_color().into_solid(),
            ),
            LspState::Failed { .. } => ("Failed", appearance.theme().ansi_fg_red()),
        };
        let mouse_states = self
            .row_mouse_states
            .borrow_mut()
            .entry((workspace_path.to_path_buf(), server_type))
            .or_default()
            .clone();

        let left = Flex::column()
            .with_spacing(4.)
            .with_child(
                appearance
                    .ui_builder()
                    .span(server.server_name())
                    .with_style(UiComponentStyles {
                        font_weight: Some(Weight::Semibold),
                        ..Default::default()
                    })
                    .build()
                    .finish(),
            )
            .with_child(
                appearance
                    .ui_builder()
                    .label(status)
                    .with_style(UiComponentStyles {
                        font_color: Some(status_color),
                        font_size: Some(12.),
                        ..Default::default()
                    })
                    .build()
                    .finish(),
            )
            .finish();

        let mut controls = Flex::row()
            .with_spacing(8.)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);

        if can_restart {
            let server = server_handle.clone();
            controls.add_child(
                appearance
                    .ui_builder()
                    .button(ButtonVariant::Secondary, mouse_states.restart)
                    .with_style(UiComponentStyles {
                        font_size: Some(12.),
                        ..Default::default()
                    })
                    .with_text_label("Restart".to_string())
                    .build()
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(CodeIndexingPageAction::RestartLspServer {
                            server: server.clone(),
                        });
                    })
                    .finish(),
            );
        }

        #[cfg(not(target_family = "wasm"))]
        {
            let log_path = lsp_log_file_path(server_type, workspace_path);
            controls.add_child(
                appearance
                    .ui_builder()
                    .button(ButtonVariant::Secondary, mouse_states.view_logs)
                    .with_style(UiComponentStyles {
                        font_size: Some(12.),
                        ..Default::default()
                    })
                    .with_text_label("View logs".to_string())
                    .build()
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(CodeIndexingPageAction::OpenLspLogs {
                            log_path: log_path.clone(),
                        });
                    })
                    .finish(),
            );
        }

        let server = server_handle.clone();
        controls.add_child(
            appearance
                .ui_builder()
                .switch(mouse_states.toggle)
                .check(is_enabled)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeIndexingPageAction::SetLspServerEnabled {
                        server: server.clone(),
                        enabled: !is_enabled,
                    });
                })
                .finish(),
        );

        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(Shrinkable::new(1., left).finish())
                .with_child(controls.finish())
                .finish(),
        )
        .with_uniform_padding(12.)
        .with_background(appearance.theme().surface_2())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish()
    }
}

impl SettingsWidget for LspManagementWidget {
    type View = CodeIndexingPageView;

    fn search_terms(&self) -> &str {
        "lsp language server runtime status restart stop start logs workspace"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let lsp_manager = LspManagerModel::as_ref(app);
        let mut workspace_roots: Vec<_> = lsp_manager.workspace_roots().cloned().collect();
        workspace_roots.sort();

        let mut column = Flex::column()
            .with_spacing(8.)
            .with_child(render_separator(appearance))
            .with_child(
                build_sub_header(appearance, "Language servers", None)
                    .with_padding_bottom(HEADER_PADDING)
                    .finish(),
            );

        if workspace_roots.is_empty() {
            column.add_child(
                appearance
                    .ui_builder()
                    .paragraph(
                        "Language servers will appear here after you open a supported project.",
                    )
                    .with_style(UiComponentStyles {
                        font_color: Some(appearance.theme().disabled_ui_text_color().into()),
                        ..Default::default()
                    })
                    .build()
                    .finish(),
            );
            return column.finish();
        }

        for workspace_path in workspace_roots {
            let Some(servers) = lsp_manager.servers_for_workspace(&workspace_path) else {
                continue;
            };
            if servers.is_empty() {
                continue;
            }

            column.add_child(
                Container::new(
                    appearance
                        .ui_builder()
                        .wrappable_text(workspace_path.to_string_lossy().into_owned(), true)
                        .with_style(UiComponentStyles {
                            font_weight: Some(Weight::Semibold),
                            font_size: Some(12.),
                            ..Default::default()
                        })
                        .build()
                        .finish(),
                )
                .with_margin_top(8.)
                .finish(),
            );

            for server in servers {
                column.add_child(self.render_server_row(&workspace_path, server, appearance, app));
            }
        }

        column.finish()
    }
}

#[cfg(not(target_family = "wasm"))]
fn lsp_log_file_path(server_type: LSPServerType, workspace_path: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(workspace_path.to_string_lossy().as_bytes());
    let workspace_hash = hex::encode(&hasher.finalize()[..8]);
    simple_logger::manager::resolve_log_path(
        "lsp",
        PathBuf::from(server_type.binary_name()).join(format!("{workspace_hash}.log")),
    )
}
