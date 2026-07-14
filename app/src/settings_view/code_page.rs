//! Local code settings: runtime LSP management, Format-on-Save, editor, and code-review controls.
//!
//! Hosted codebase indexing and its team/plan policy UI intentionally do not live here.

#[cfg(feature = "local_fs")]
use super::features::external_editor::ExternalEditorView;
use super::{
    settings_page::{
        build_sub_header, render_body_item, render_separator, MatchData, PageType,
        SettingsPageMeta, SettingsPageViewHandle, SettingsWidget, HEADER_PADDING,
    },
    LocalOnlyIconState, SettingsAction, SettingsSection, ToggleState,
};
use crate::{
    appearance::Appearance, settings::CodeSettings, terminal::general_settings::GeneralSettings,
    workspace::tab_settings::TabSettings,
};
use ai::project_context::model::{ProjectContextModel, ProjectContextModelEvent};
use lsp::supported_servers::LSPServerType;
use lsp::{LspManagerModel, LspManagerModelEvent, LspServerModel, LspState};

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use warp_core::{features::FeatureFlag, settings::ToggleableSetting as _};
use warp_errors::report_if_error;
use warpui::{
    elements::{
        ChildView, Container, CornerRadius, CrossAxisAlignment, Element, Empty, Flex,
        MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, Radius, Shrinkable,
    },
    fonts::Weight,
    keymap::ContextPredicate,
    platform::Cursor,
    ui_components::{
        button::ButtonVariant,
        components::{UiComponent, UiComponentStyles},
        switch::SwitchStateHandle,
    },
    Action, AppContext, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

pub struct CodeSettingsPageView {
    page: PageType<Self>,
    #[cfg(feature = "local_fs")]
    external_editor_view: Option<ViewHandle<ExternalEditorView>>,
}

impl CodeSettingsPageView {
    pub fn new(ctx: &mut ViewContext<CodeSettingsPageView>) -> Self {
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

        let (page, external_editor_view) = Self::build_page(ctx);

        Self {
            page,
            #[cfg(feature = "local_fs")]
            external_editor_view,
        }
    }

    /// Builds the page widgets. Code is now a single page (no subpages, no category titles),
    /// displaying the "Editor and Code Review" toggles laid out flat.
    #[cfg(feature = "local_fs")]
    fn build_page(
        ctx: &mut ViewContext<Self>,
    ) -> (PageType<Self>, Option<ViewHandle<ExternalEditorView>>) {
        let (widgets, external_editor_view) = if FeatureFlag::ZapNewSettingsModes.is_enabled() {
            let editor_view = ctx.add_typed_action_view(ExternalEditorView::new);
            let widgets: Vec<Box<dyn SettingsWidget<View = Self>>> = vec![
                Box::new(LspManagementWidget::default()),
                Box::new(ExternalEditorCodeWidget),
                Box::new(AutoOpenCodeReviewPaneCodeWidget::default()),
                Box::new(CodeReviewPanelToggleWidget::default()),
                Box::new(CodeReviewDiffStatsToggleWidget::default()),
                Box::new(ProjectExplorerToggleWidget::default()),
                Box::new(GlobalSearchToggleWidget::default()),
                Box::new(FormatOnSaveToggleWidget::default()),
            ];
            (widgets, Some(editor_view))
        } else {
            // legacy view: under the old settings mode the Code page renders nothing (the original
            // CodePageWidget only rendered an LSP-era header with no real meaning, so just return an empty page).
            (vec![], None)
        };
        (
            PageType::new_uncategorized(widgets, None),
            external_editor_view,
        )
    }

    /// Under wasm builds there is no ExternalEditorView; only the 4 non-external-editor toggles are rendered.
    #[cfg(not(feature = "local_fs"))]
    fn build_page(
        _ctx: &mut ViewContext<Self>,
    ) -> (PageType<Self>, Option<ViewHandle<ExternalEditorView>>) {
        let widgets: Vec<Box<dyn SettingsWidget<View = Self>>> =
            if FeatureFlag::ZapNewSettingsModes.is_enabled() {
                vec![
                    Box::new(LspManagementWidget::default()),
                    Box::new(AutoOpenCodeReviewPaneCodeWidget::default()),
                    Box::new(CodeReviewPanelToggleWidget::default()),
                    Box::new(CodeReviewDiffStatsToggleWidget::default()),
                    Box::new(ProjectExplorerToggleWidget::default()),
                    Box::new(GlobalSearchToggleWidget::default()),
                    Box::new(FormatOnSaveToggleWidget::default()),
                ]
            } else {
                vec![]
            };
        (PageType::new_uncategorized(widgets, None), None)
    }
}

impl Entity for CodeSettingsPageView {
    type Event = CodeSettingsPageEvent;
}

impl View for CodeSettingsPageView {
    fn ui_name() -> &'static str {
        "CodePage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

#[derive(Debug, Clone)]
pub enum CodeSettingsPageEvent {
    OpenProjectRules { rule_paths: Vec<PathBuf> },
}

#[derive(Debug, Clone)]
pub enum CodeSettingsPageAction {
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
    ToggleCodeReviewPanel,
    ToggleShowCodeReviewDiffStats,
    ToggleAutoOpenCodeReviewPane,
    ToggleProjectExplorer,
    ToggleGlobalSearch,
    ToggleFormatOnSave,
}

impl TypedActionView for CodeSettingsPageView {
    type Action = CodeSettingsPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CodeSettingsPageAction::OpenProjectRules { rule_paths } => {
                ctx.emit(CodeSettingsPageEvent::OpenProjectRules {
                    rule_paths: rule_paths.clone(),
                });
            }
            CodeSettingsPageAction::SetLspServerEnabled { server, enabled } => {
                server.update(ctx, |server, ctx| {
                    if *enabled {
                        report_if_error!(server.manual_start(ctx));
                    } else {
                        report_if_error!(server.stop(true, ctx));
                    }
                });
                ctx.notify();
            }
            CodeSettingsPageAction::RestartLspServer { server } => {
                server.update(ctx, |server, ctx| {
                    server.restart(ctx);
                });
                ctx.notify();
            }
            CodeSettingsPageAction::OpenLspLogs { log_path } => {
                ctx.open_file_path(log_path);
            }
            CodeSettingsPageAction::ToggleCodeReviewPanel => {
                TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.show_code_review_button.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::ToggleShowCodeReviewDiffStats => {
                TabSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .show_code_review_diff_stats
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::ToggleProjectExplorer => {
                CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.show_project_explorer.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::ToggleGlobalSearch => {
                CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.show_global_search.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::ToggleFormatOnSave => {
                CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.format_on_save.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            CodeSettingsPageAction::ToggleAutoOpenCodeReviewPane => {
                GeneralSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .auto_open_code_review_pane_on_first_agent_change
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
        }
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
                ("Busy", appearance.theme().ansi_fg_yellow().into())
            }
            LspState::Available { .. } => ("Available", appearance.theme().ansi_fg_green().into()),
            LspState::Starting => ("Starting", appearance.theme().ansi_fg_yellow().into()),
            LspState::Stopping { .. } => ("Stopping", appearance.theme().ansi_fg_yellow().into()),
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
            LspState::Failed { .. } => ("Failed", appearance.theme().ansi_fg_red().into()),
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
                        ctx.dispatch_typed_action(CodeSettingsPageAction::RestartLspServer {
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
                        ctx.dispatch_typed_action(CodeSettingsPageAction::OpenLspLogs {
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
                    ctx.dispatch_typed_action(CodeSettingsPageAction::SetLspServerEnabled {
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
    type View = CodeSettingsPageView;

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

#[cfg(feature = "local_fs")]
struct ExternalEditorCodeWidget;

#[cfg(feature = "local_fs")]
impl SettingsWidget for ExternalEditorCodeWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "code editor open files markdown AI conversations layout pane tab"
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        if let Some(editor_view) = &view.external_editor_view {
            ChildView::new(editor_view).finish()
        } else {
            Empty::new().finish()
        }
    }
}

#[derive(Default)]
struct AutoOpenCodeReviewPaneCodeWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for AutoOpenCodeReviewPaneCodeWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "oz auto open code review pane panel agent mode change first time accepted diff view conversation"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let general_settings = GeneralSettings::as_ref(app);
        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-auto-open-review-panel"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*general_settings.auto_open_code_review_pane_on_first_agent_change)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleAutoOpenCodeReviewPane);
                })
                .finish(),
            Some(crate::t!("settings-code-auto-open-review-panel-desc")),
        )
    }
}

impl SettingsPageMeta for CodeSettingsPageView {
    fn section() -> SettingsSection {
        SettingsSection::Code
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

impl From<ViewHandle<CodeSettingsPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<CodeSettingsPageView>) -> Self {
        SettingsPageViewHandle::Code(view_handle)
    }
}

#[derive(Default)]
struct CodeReviewPanelToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for CodeReviewPanelToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "code review panel right side diff git"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let tab_settings = TabSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-show-code-review-button"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*tab_settings.show_code_review_button)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleCodeReviewPanel);
                })
                .finish(),
            Some(crate::t!("settings-code-show-code-review-button-desc")),
        )
    }
}

#[derive(Default)]
struct CodeReviewDiffStatsToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for CodeReviewDiffStatsToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "code review diff stats lines added removed counts"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let tab_settings = TabSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-show-diff-stats"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*tab_settings.show_code_review_diff_stats)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(
                        CodeSettingsPageAction::ToggleShowCodeReviewDiffStats,
                    );
                })
                .finish(),
            Some(crate::t!("settings-code-show-diff-stats-desc")),
        )
    }
}

#[derive(Default)]
struct ProjectExplorerToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for ProjectExplorerToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "project explorer file tree left panel tools"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let code_settings = CodeSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-project-explorer"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*code_settings.show_project_explorer)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleProjectExplorer);
                })
                .finish(),
            Some(crate::t!("settings-code-project-explorer-desc")),
        )
    }
}

#[derive(Default)]
struct GlobalSearchToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for GlobalSearchToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "global search file search left panel tools"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let code_settings = CodeSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            crate::t!("settings-code-global-search"),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*code_settings.show_global_search)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleGlobalSearch);
                })
                .finish(),
            Some(crate::t!("settings-code-global-search-desc")),
        )
    }
}

#[derive(Default)]
struct FormatOnSaveToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for FormatOnSaveToggleWidget {
    type View = CodeSettingsPageView;

    fn search_terms(&self) -> &str {
        "format on save lsp language server formatting reformat editor"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let code_settings = CodeSettings::as_ref(app);

        render_body_item::<CodeSettingsPageAction>(
            "Format on save (requires an active language server)".into(),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*code_settings.format_on_save)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(CodeSettingsPageAction::ToggleFormatOnSave);
                })
                .finish(),
            Some(
                "Only applies when a language server is active for the file. Automatically formats the file with the language server on save; other LSP features (hover, go-to-definition, references, diagnostics) are unaffected."
                    .into(),
            ),
        )
    }
}
