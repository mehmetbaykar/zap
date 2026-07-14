//! [`RootTuiView`]: the root view of the `warp-tui` front-end.

use warp::tui_export::TerminalSurfaceInit;
use warpui_core::elements::tui::{TuiChildView, TuiElement};
use warpui_core::keymap::macros::*;
use warpui_core::keymap::FixedBinding;
use warpui_core::platform::TerminationMode;
use warpui_core::{
    keymap, AppContext, Entity, EntityId, SingletonEntity, TuiView, TypedActionView, ViewContext,
    ViewHandle,
};

use crate::keybindings::TUI_BINDING_GROUP;
use crate::terminal_session_view::TuiTerminalSessionView;
use crate::ui::terminal_starting;

/// Whether the terminal session has been created yet.
enum RootTuiState {
    /// No terminal session exists yet.
    Starting,
    /// The live terminal session.
    Terminal(ViewHandle<TuiTerminalSessionView>),
}

/// Typed actions handled by [`RootTuiView`].
#[derive(Debug, Clone)]
pub enum RootTuiAction {
    /// Exit the app. Bound to ctrl-c in the root's keymap context; the
    /// terminal session's deeper `Interrupt` binding wins while a session
    /// exists, so this fires only before the terminal session is mounted.
    ExitApp,
}

/// The app-level TUI shell.
pub struct RootTuiView {
    state: RootTuiState,
}

/// Registers the root view's keybindings. Called once at TUI startup from
/// `keybindings::init`.
pub fn init(app: &mut AppContext) {
    app.register_fixed_bindings([FixedBinding::new(
        "ctrl-c",
        RootTuiAction::ExitApp,
        id!(RootTuiView::ui_name()),
    )
    .with_group(TUI_BINDING_GROUP)]);
}

impl RootTuiView {
    pub(crate) fn new() -> Self {
        Self {
            state: RootTuiState::Starting,
        }
    }
    /// Creates the terminal child view, or returns the existing one if it was
    /// already created. Callers notify the root so it re-renders from the
    /// startup placeholder to the terminal session.
    pub(crate) fn create_terminal_session(
        &mut self,
        surface_init: TerminalSurfaceInit,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<TuiTerminalSessionView> {
        if let RootTuiState::Terminal(terminal_session) = &self.state {
            return terminal_session.clone();
        }
        let terminal_session =
            ctx.add_typed_action_tui_view(|ctx| TuiTerminalSessionView::new(surface_init, ctx));
        self.state = RootTuiState::Terminal(terminal_session.clone());
        terminal_session
    }
}

impl Entity for RootTuiView {
    type Event = ();
}

impl TuiView for RootTuiView {
    fn ui_name() -> &'static str {
        "RootTuiView"
    }

    fn child_view_ids(&self, _ctx: &AppContext) -> Vec<EntityId> {
        // The TUI runtime uses this for child focus and event routing; only the
        // live terminal session participates.
        match &self.state {
            RootTuiState::Terminal(terminal_session) => vec![terminal_session.id()],
            RootTuiState::Starting => Vec::new(),
        }
    }

    fn render(&self, _: &AppContext) -> Box<dyn TuiElement> {
        match &self.state {
            RootTuiState::Terminal(terminal_session) => {
                TuiChildView::new(terminal_session).finish()
            }
            RootTuiState::Starting => terminal_starting(),
        }
    }

    fn keymap_context(&self, _ctx: &AppContext) -> keymap::Context {
        // Propagate focus context into the input view so keystrokes reach it.
        let mut context = keymap::Context::default();
        context.set.insert("RootTuiView");
        context
    }
}

impl TypedActionView for RootTuiView {
    type Action = RootTuiAction;

    fn handle_action(&mut self, action: &RootTuiAction, ctx: &mut ViewContext<Self>) {
        match action {
            RootTuiAction::ExitApp => ctx.terminate_app(TerminationMode::ForceTerminate, None),
        }
    }
}
