use warp_editor::content::buffer::InitialBufferState;
use warp_editor::render::element::VerticalExpansionBehavior;
use warp_util::path::LineAndColumnArg;
use warpui::elements::{ChildView, MouseStateHandle};
use warpui::text_layout::ClipConfig;
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::appearance::Appearance;
use crate::code::editor::scroll::{ScrollPosition, ScrollTrigger};
use crate::code::editor::view::{CodeEditorRenderOptions, CodeEditorView};
use crate::editor::InteractionState;
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::pane::view::{self, HeaderContent, StandardHeader, StandardHeaderOptions};
use crate::pane_group::{BackingView, PaneConfiguration, PaneEvent, PaneHeaderAction};
use crate::server::network_logging::NetworkLogModel;
use crate::ui_components::buttons::icon_button_with_color;
use crate::ui_components::{blended_colors, icons};

pub const NETWORK_LOG_HEADER_TEXT: &str = "Network log";

const REFRESH_TOOLTIP: &str = "Refresh";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkLogViewEvent {
    Pane(PaneEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkLogViewAction {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkLogViewCustomAction {
    Refresh,
}

pub struct NetworkLogView {
    editor: ViewHandle<CodeEditorView>,
    pane_configuration: ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,
    refresh_button_mouse_state: MouseStateHandle,
}

impl NetworkLogView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let pane_configuration =
            ctx.add_model(|_ctx| PaneConfiguration::new(NETWORK_LOG_HEADER_TEXT));
        let snapshot = NetworkLogModel::as_ref(ctx).snapshot_text();
        let editor = ctx.add_typed_action_view(|ctx| {
            let mut view = CodeEditorView::new(
                None,
                None,
                CodeEditorRenderOptions::new(VerticalExpansionBehavior::FillMaxHeight),
                ctx,
            );
            Self::apply_snapshot_to_editor(&mut view, &snapshot, ctx);
            view.set_interaction_state(InteractionState::Selectable, ctx);
            view
        });

        Self {
            editor,
            pane_configuration,
            focus_handle: None,
            refresh_button_mouse_state: MouseStateHandle::default(),
        }
    }

    pub fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    pub fn focus(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus(&self.editor);
    }

    pub fn reload_snapshot(&self, ctx: &mut ViewContext<Self>) {
        let snapshot = NetworkLogModel::as_ref(ctx).snapshot_text();
        self.editor.update(ctx, |view, ctx| {
            Self::apply_snapshot_to_editor(view, &snapshot, ctx);
        });
    }

    fn apply_snapshot_to_editor(
        view: &mut CodeEditorView,
        snapshot: &str,
        ctx: &mut ViewContext<CodeEditorView>,
    ) {
        view.reset(InitialBufferState::plain_text(snapshot), ctx);
        let version = view.buffer_version(ctx);
        view.set_pending_scroll(ScrollTrigger::new(
            ScrollPosition::LineAndColumn(LineAndColumnArg {
                line_num: 1,
                column_num: Some(0),
            }),
            version,
        ));
    }

    fn render_refresh_button(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let ui_builder = appearance.ui_builder().clone();

        icon_button_with_color(
            appearance,
            icons::Icon::Refresh,
            false,
            self.refresh_button_mouse_state.clone(),
            blended_colors::text_sub(theme, theme.background()).into(),
        )
        .with_tooltip(move || {
            ui_builder
                .tool_tip(REFRESH_TOOLTIP.to_string())
                .build()
                .finish()
        })
        .build()
        .on_click(|ctx, _, _| {
            ctx.dispatch_typed_action::<
                PaneHeaderAction<NetworkLogViewAction, NetworkLogViewCustomAction>,
            >(PaneHeaderAction::CustomAction(
                NetworkLogViewCustomAction::Refresh,
            ));
        })
        .finish()
    }
}

impl Entity for NetworkLogView {
    type Event = NetworkLogViewEvent;
}

impl View for NetworkLogView {
    fn ui_name() -> &'static str {
        "NetworkLogView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        ChildView::new(&self.editor).finish()
    }
}

impl TypedActionView for NetworkLogView {
    type Action = NetworkLogViewAction;

    fn handle_action(&mut self, action: &Self::Action, _ctx: &mut ViewContext<Self>) {
        match *action {}
    }
}

impl BackingView for NetworkLogView {
    type PaneHeaderOverflowMenuAction = NetworkLogViewAction;
    type CustomAction = NetworkLogViewCustomAction;
    type AssociatedData = ();

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        action: &Self::PaneHeaderOverflowMenuAction,
        _ctx: &mut ViewContext<Self>,
    ) {
        match *action {}
    }

    fn handle_custom_action(
        &mut self,
        custom_action: &Self::CustomAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match custom_action {
            NetworkLogViewCustomAction::Refresh => self.reload_snapshot(ctx),
        }
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(NetworkLogViewEvent::Pane(PaneEvent::Close));
    }

    fn focus_contents(&mut self, ctx: &mut ViewContext<Self>) {
        self.focus(ctx);
    }

    fn render_header_content(
        &self,
        _ctx: &view::HeaderRenderContext<'_>,
        app: &AppContext,
    ) -> HeaderContent {
        HeaderContent::Standard(StandardHeader {
            title: NETWORK_LOG_HEADER_TEXT.to_string(),
            title_secondary: None,
            title_style: None,
            title_clip_config: ClipConfig::start(),
            title_max_width: None,
            left_of_title: None,
            right_of_title: None,
            left_of_overflow: Some(self.render_refresh_button(app)),
            options: StandardHeaderOptions {
                always_show_icons: true,
                ..StandardHeaderOptions::default()
            },
        })
    }

    fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, _ctx: &mut ViewContext<Self>) {
        self.focus_handle = Some(focus_handle);
    }
}
