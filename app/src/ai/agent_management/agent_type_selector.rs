//! Agent type selector modal.
//!
//! Upstream lets users choose between a cloud agent and a local agent here. Zap has no cloud
//! agents (no accounts, no cloud sync), so the `Cloud` option and its associated selection state
//! are removed; only the local-agent option remains. The modal is kept (rather than having "New
//! agent" skip straight to spawning) because it still carries a short description of what a local
//! agent run is, and keeping it avoids a second divergence point in `view.rs`'s click handling.
//! Multi-option keyboard navigation (up/down arrow to move between options) is dropped since
//! there is exactly one option to select.

use pathfinder_color::ColorU;
use pathfinder_geometry::vector::vec2f;
use warp_core::ui::color::blend::Blend;
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    Align, Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Dismiss,
    DropShadow, Element, Flex, Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle,
    ParentElement, Radius, Shrinkable, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::{FixedBinding, Keystroke};
use warpui::platform::Cursor;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext};

use crate::appearance::Appearance;
use crate::ui_components::icons::Icon;

// Modal dimensions based on Figma design.
const MODAL_WIDTH: f32 = 440.;
const DIALOG_CORNER_RADIUS: f32 = 8.;

const HEADER_PADDING_TOP: f32 = 24.;
const HEADER_PADDING_BOTTOM: f32 = 12.;
const HEADER_PADDING_HORIZONTAL: f32 = 24.;

const BODY_PADDING_VERTICAL: f32 = 16.;
const BODY_PADDING_HORIZONTAL: f32 = 20.;

const OPTION_PADDING_VERTICAL: f32 = 8.;
const OPTION_PADDING_HORIZONTAL: f32 = 12.;
const OPTION_CORNER_RADIUS: f32 = 4.;
const OPTION_GAP: f32 = 12.;

const AVATAR_SIZE: f32 = 48.;
const AVATAR_ICON_SIZE: f32 = 24.;

const TITLE_FONT_SIZE: f32 = 16.;
const OPTION_TITLE_FONT_SIZE: f32 = 14.;
const OPTION_DESC_FONT_SIZE: f32 = 12.;

/// The type of agent selected by the user.
///
/// Zap only ever runs local agents; this remains an enum (rather than a unit struct) to keep
/// `view.rs`'s `AgentTypeSelectorEvent::Selected(AgentType)` match exhaustive and easy to extend
/// if a second local execution mode is added later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentType {
    /// Local agent - runs on the user's machine.
    Local,
}

#[derive(Debug, Clone)]
pub enum AgentTypeSelectorAction {
    SelectLocalAgent,
    Dismiss,
    Enter,
}

#[derive(Debug)]
pub enum AgentTypeSelectorEvent {
    Selected(AgentType),
    Dismissed,
}

pub struct AgentTypeSelector {
    close_button_mouse_state: MouseStateHandle,
    local_agent_mouse_state: MouseStateHandle,
    dialog_mouse_state: MouseStateHandle,
}

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings(vec![
        FixedBinding::new(
            "escape",
            AgentTypeSelectorAction::Dismiss,
            id!("AgentTypeSelector"),
        ),
        FixedBinding::new(
            "enter",
            AgentTypeSelectorAction::Enter,
            id!("AgentTypeSelector"),
        ),
        FixedBinding::new(
            "numpadenter",
            AgentTypeSelectorAction::Enter,
            id!("AgentTypeSelector"),
        ),
    ]);
}

impl AgentTypeSelector {
    pub fn new(_ctx: &mut ViewContext<Self>) -> Self {
        Self {
            close_button_mouse_state: MouseStateHandle::default(),
            local_agent_mouse_state: MouseStateHandle::default(),
            dialog_mouse_state: MouseStateHandle::default(),
        }
    }

    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();

        let title = Text::new(
            "Start a new agent".to_string(),
            appearance.ui_font_family(),
            TITLE_FONT_SIZE,
        )
        .with_style(Properties::default().weight(Weight::Bold))
        .with_color(theme.active_ui_text_color().into())
        .soft_wrap(true)
        .finish();

        let close_button = appearance
            .ui_builder()
            .close_button(16., self.close_button_mouse_state.clone())
            .build()
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(AgentTypeSelectorAction::Dismiss);
            })
            .finish();

        let esc_keystroke = Keystroke::parse("escape").expect("escape keystroke parses");
        let esc_pill = appearance
            .ui_builder()
            .keyboard_shortcut(&esc_keystroke)
            .with_style(UiComponentStyles {
                font_size: Some(OPTION_DESC_FONT_SIZE),
                font_color: Some(theme.nonactive_ui_text_color().into_solid()),
                border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.))),
                padding: Some(Coords {
                    top: 0.,
                    bottom: 0.,
                    left: 3.,
                    right: 3.,
                }),
                height: Some(16.),
                ..Default::default()
            })
            .build()
            .finish();

        let right_controls = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(close_button)
            .with_child(Container::new(esc_pill).with_margin_left(4.).finish())
            .finish();

        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(12.)
            .with_child(Shrinkable::new(1., title).finish())
            .with_child(right_controls)
            .finish()
    }

    fn render_option(
        &self,
        icon: Icon,
        title: &'static str,
        description: &'static str,
        mouse_state: MouseStateHandle,
        action: AgentTypeSelectorAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();

        let font_family = appearance.ui_font_family();
        let active_text = theme.active_ui_text_color();
        let nonactive_text = theme.nonactive_ui_text_color();

        let base_background = theme.surface_2();
        let hover_background = base_background.blend(&internal_colors::accent_overlay_1(theme));

        let base_border = internal_colors::neutral_4(theme);
        let hover_border = theme.accent().into_solid();

        let avatar_background = internal_colors::neutral_2(theme);
        let avatar_border = internal_colors::neutral_3(theme);

        let icon_color = nonactive_text;

        let action = action.clone();
        Hoverable::new(mouse_state, move |state| {
            let is_hovered = state.is_hovered() || state.is_clicked();
            let (background, border_color) = if is_hovered {
                (hover_background, hover_border)
            } else {
                (base_background, base_border)
            };

            let avatar_icon = ConstrainedBox::new(icon.to_warpui_icon(icon_color).finish())
                .with_width(AVATAR_ICON_SIZE)
                .with_height(AVATAR_ICON_SIZE)
                .finish();

            let avatar_contents = ConstrainedBox::new(
                Flex::row()
                    .with_main_axis_alignment(MainAxisAlignment::Center)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_child(avatar_icon)
                    .finish(),
            )
            .with_width(AVATAR_SIZE)
            .with_height(AVATAR_SIZE)
            .finish();

            let avatar = Container::new(avatar_contents)
                .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                .with_background(avatar_background)
                .with_border(Border::all(1.).with_border_color(avatar_border))
                .finish();

            let title_text = Text::new(title.to_string(), font_family, OPTION_TITLE_FONT_SIZE)
                .with_style(Properties::default().weight(Weight::Semibold))
                .with_color(active_text.into())
                .finish();

            let title_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.)
                .with_child(title_text);

            let description_text =
                Text::new(description.to_string(), font_family, OPTION_DESC_FONT_SIZE)
                    .with_style(Properties::default().weight(Weight::Normal))
                    .with_color(nonactive_text.into())
                    .soft_wrap(true)
                    .finish();

            let text_content = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_child(title_row.finish())
                .with_child(
                    Container::new(description_text)
                        .with_margin_top(4.)
                        .finish(),
                )
                .finish();

            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(OPTION_GAP)
                    .with_child(avatar)
                    .with_child(Shrinkable::new(1., text_content).finish())
                    .finish(),
            )
            .with_padding_left(OPTION_PADDING_HORIZONTAL)
            .with_padding_right(OPTION_PADDING_HORIZONTAL)
            .with_padding_top(OPTION_PADDING_VERTICAL)
            .with_padding_bottom(OPTION_PADDING_VERTICAL)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(OPTION_CORNER_RADIUS)))
            .with_border(Border::all(1.).with_border_color(border_color))
            .with_background(background)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(action.clone());
        })
        .finish()
    }

    fn render_modal(&self, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
        let theme = appearance.theme();

        let header = Container::new(self.render_header(appearance))
            .with_padding_top(HEADER_PADDING_TOP)
            .with_padding_bottom(HEADER_PADDING_BOTTOM)
            .with_padding_left(HEADER_PADDING_HORIZONTAL)
            .with_padding_right(HEADER_PADDING_HORIZONTAL)
            .finish();

        let local_agent_option = self.render_option(
            Icon::Oz,
            "Local agent",
            "Runs on your machine and requires supervision. Best for quick, interactive tasks.",
            self.local_agent_mouse_state.clone(),
            AgentTypeSelectorAction::SelectLocalAgent,
            appearance,
        );

        let options = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(local_agent_option)
            .finish();

        let body = Container::new(options)
            .with_padding_top(BODY_PADDING_VERTICAL)
            .with_padding_bottom(BODY_PADDING_VERTICAL)
            .with_padding_left(BODY_PADDING_HORIZONTAL)
            .with_padding_right(BODY_PADDING_HORIZONTAL)
            .finish();

        let dialog_contents = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header)
            .with_child(body)
            .finish();

        let dialog_background = theme
            .surface_1()
            .blend(&internal_colors::fg_overlay_1(theme));
        let dialog_border = internal_colors::neutral_4(theme);

        let dialog = Container::new(dialog_contents)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(DIALOG_CORNER_RADIUS)))
            .with_border(Border::all(1.).with_border_color(dialog_border))
            .with_background(dialog_background)
            .with_drop_shadow(DropShadow {
                color: ColorU::new(0, 0, 0, 77),
                offset: vec2f(0., 7.),
                blur_radius: 7.,
                spread_radius: 0.,
            })
            .finish();

        // Wrap dialog in a Hoverable to consume click events within the modal,
        // preventing the Dismiss from triggering when clicking inside the dialog.
        let clickable_dialog =
            Hoverable::new(self.dialog_mouse_state.clone(), |_| dialog).on_click(|_, _, _| {});

        let constrained_dialog = ConstrainedBox::new(clickable_dialog.finish())
            .with_width(MODAL_WIDTH)
            .finish();

        let dismiss_dialog = Dismiss::new(constrained_dialog)
            .prevent_interaction_with_other_elements()
            .on_dismiss(|ctx, _app| {
                ctx.dispatch_typed_action(AgentTypeSelectorAction::Dismiss);
            })
            .finish();

        Container::new(Align::new(dismiss_dialog).finish())
            .with_background_color(ColorU::new(0, 0, 0, 179))
            .with_corner_radius(app.windows().window_corner_radius())
            .finish()
    }
}

impl Entity for AgentTypeSelector {
    type Event = AgentTypeSelectorEvent;
}

impl TypedActionView for AgentTypeSelector {
    type Action = AgentTypeSelectorAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            AgentTypeSelectorAction::SelectLocalAgent => {
                ctx.emit(AgentTypeSelectorEvent::Selected(AgentType::Local));
                ctx.notify();
            }
            AgentTypeSelectorAction::Dismiss => {
                ctx.emit(AgentTypeSelectorEvent::Dismissed);
                ctx.notify();
            }
            AgentTypeSelectorAction::Enter => {
                ctx.emit(AgentTypeSelectorEvent::Selected(AgentType::Local));
                ctx.notify();
            }
        }
    }
}

impl View for AgentTypeSelector {
    fn ui_name() -> &'static str {
        "AgentTypeSelector"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        self.render_modal(appearance, app)
    }
}
