use ui_components::{Component as _, Options as _, button, tooltip};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::Fill;
use warp_core::ui::theme::color::internal_colors;
use warpui_core::elements::{
    Border, ClippedScrollStateHandle, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    Flex, FormattedTextElement, Hoverable, MainAxisSize, MouseStateHandle, ParentElement, Radius,
};
use warpui_core::fonts::Weight;
use warpui_core::keymap::Keystroke;
use warpui_core::platform::Cursor;
use warpui_core::prelude::Align;
use warpui_core::text_layout::TextAlignment;
use warpui_core::ui_components::components::{UiComponent as _, UiComponentStyles};
use warpui_core::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity as _, TypedActionView, View,
    ViewContext,
};

use super::OnboardingSlide;
use crate::model::{AiAccessChoice, NoAiConfirmationSource, OnboardingStateModel};
use crate::slides::{bottom_nav, layout, slide_content};

#[derive(Debug, Clone)]
pub enum AiAccessSlideAction {
    SelectByok,
    AddApiKeyClicked,
    AddCustomEndpointClicked,
    BackClicked,
    NextClicked,
    NoAiClicked,
}

/// Emitted to the parent onboarding view so the (app-crate) settings modals can be
/// hosted at the root level — the onboarding crate can't reference them directly.
#[derive(Debug, Clone)]
pub enum AiAccessSlideEvent {
    AddApiKeyRequested,
    AddCustomEndpointRequested,
}

/// Configures accountless bring-your-own-key or custom-endpoint inference.
pub struct AiAccessSlide {
    onboarding_state: ModelHandle<OnboardingStateModel>,
    byok_mouse_state: MouseStateHandle,
    add_key_button: button::Button,
    add_endpoint_button: button::Button,
    back_button: button::Button,
    next_button: button::Button,
    no_ai_button: button::Button,
    scroll_state: ClippedScrollStateHandle,
    /// How many BYOK provider keys and custom endpoints the user has configured
    /// (mirrors the app's `ApiKeyManager`). Drives the "N keys connected" status
    /// line and gates "Next" on the bring-your-own path.
    byok_key_count: usize,
    byok_endpoint_count: usize,
}

impl AiAccessSlide {
    pub(crate) fn new(onboarding_state: ModelHandle<OnboardingStateModel>) -> Self {
        Self {
            onboarding_state,
            byok_mouse_state: MouseStateHandle::default(),
            add_key_button: button::Button::default(),
            add_endpoint_button: button::Button::default(),
            back_button: button::Button::default(),
            next_button: button::Button::default(),
            no_ai_button: button::Button::default(),
            scroll_state: ClippedScrollStateHandle::new(),
            byok_key_count: 0,
            byok_endpoint_count: 0,
        }
    }

    // The final DES-816 visual exports have not landed yet, so the right panel
    // reuses the existing bundled agent welcome image.
    pub(crate) const VISUAL_IMAGE_PATHS: &'static [&'static str] =
        &["async/png/onboarding/welcome_agent.png"];

    fn choice(&self, app: &AppContext) -> AiAccessChoice {
        self.onboarding_state.as_ref(app).ai_access_choice()
    }

    /// At least one locally configured key or endpoint is required to continue.
    fn can_advance(&self) -> bool {
        self.byok_key_count > 0 || self.byok_endpoint_count > 0
    }

    fn render_content(
        &self,
        appearance: &Appearance,
        choice: AiAccessChoice,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let bottom_nav = Align::new(self.render_bottom_nav(appearance, app)).finish();

        slide_content::onboarding_slide_content(
            vec![
                Align::new(self.render_header(appearance)).left().finish(),
                Align::new(self.render_options(appearance, choice)).finish(),
            ],
            bottom_nav,
            self.scroll_state.clone(),
            appearance,
        )
    }

    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();

        let title = appearance
            .ui_builder()
            .paragraph("Choose how to access AI")
            .with_style(UiComponentStyles {
                font_size: Some(36.),
                font_weight: Some(Weight::Medium),
                ..Default::default()
            })
            .build()
            .finish();

        let subtitle = FormattedTextElement::from_str(
            "Connect your own provider key or OpenAI-compatible endpoint.",
            appearance.ui_font_family(),
            16.,
        )
        .with_color(internal_colors::text_sub(
            theme,
            theme.background().into_solid(),
        ))
        .with_weight(Weight::Normal)
        .with_alignment(TextAlignment::Left)
        .with_line_height_ratio(1.0)
        .finish();

        Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(title)
            .with_child(Container::new(subtitle).with_margin_top(16.).finish())
            .finish()
    }

    fn render_options(&self, appearance: &Appearance, choice: AiAccessChoice) -> Box<dyn Element> {
        let byok_card = self.render_byok_card(appearance, matches!(choice, AiAccessChoice::Byok));

        Container::new(byok_card).with_margin_top(38.).finish()
    }

    /// Shared chrome for an option card: selected/unselected background + border,
    /// hover/click to select.
    fn render_card_chrome(
        appearance: &Appearance,
        is_selected: bool,
        mouse_state: MouseStateHandle,
        select_action: AiAccessSlideAction,
        content: Box<dyn Element>,
    ) -> Box<dyn Element> {
        const RADIUS: f32 = 8.;

        let theme = appearance.theme();
        let background = if is_selected {
            Some(internal_colors::accent_overlay_1(theme))
        } else {
            None
        };
        let border_color = if is_selected {
            theme.accent()
        } else {
            Fill::Solid(internal_colors::neutral_4(theme))
        };

        Hoverable::new(mouse_state, move |_| {
            let mut container = Container::new(content)
                .with_uniform_padding(24.)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(RADIUS)))
                .with_border(Border::all(1.).with_border_fill(border_color));
            if let Some(bg) = background {
                container = container.with_background(bg);
            }
            container.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(select_action.clone());
        })
        .finish()
    }

    fn render_byok_card(&self, appearance: &Appearance, is_selected: bool) -> Box<dyn Element> {
        let theme = appearance.theme();
        let bg_solid = theme.background().into_solid();
        let label_color = if is_selected {
            internal_colors::text_main(theme, bg_solid)
        } else {
            internal_colors::text_sub(theme, bg_solid)
        };
        let description_color = internal_colors::text_sub(theme, bg_solid);

        let label = appearance
            .ui_builder()
            .paragraph("Use my own key or endpoint")
            .with_style(UiComponentStyles {
                font_size: Some(16.),
                font_weight: Some(Weight::Semibold),
                font_color: Some(label_color),
                ..Default::default()
            })
            .build()
            .finish();

        let description = FormattedTextElement::from_str(
            "Keys stay on this device and requests go directly to your provider.",
            appearance.ui_font_family(),
            14.,
        )
        .with_color(description_color)
        .with_weight(Weight::Normal)
        .with_alignment(TextAlignment::Left)
        .with_line_height_ratio(1.2)
        .finish();

        let add_key_button = self.add_key_button.render(
            appearance,
            button::Params {
                content: button::Content::Label("+ Add key".into()),
                theme: &button::themes::Secondary,
                options: button::Options {
                    on_click: Some(Box::new(|ctx, _app, _pos| {
                        ctx.dispatch_typed_action(AiAccessSlideAction::AddApiKeyClicked);
                    })),
                    ..button::Options::default(appearance)
                },
            },
        );

        let add_endpoint_button = self.add_endpoint_button.render(
            appearance,
            button::Params {
                content: button::Content::Label("+ Add custom endpoint".into()),
                theme: &button::themes::Secondary,
                options: button::Options {
                    on_click: Some(Box::new(|ctx, _app, _pos| {
                        ctx.dispatch_typed_action(AiAccessSlideAction::AddCustomEndpointClicked);
                    })),
                    ..button::Options::default(appearance)
                },
            },
        );

        let buttons_row = Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(add_key_button)
            .with_child(
                Container::new(add_endpoint_button)
                    .with_margin_left(8.)
                    .finish(),
            )
            .finish();

        let mut content = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(label)
            .with_child(Container::new(description).with_margin_top(12.).finish())
            .with_child(Container::new(buttons_row).with_margin_top(16.).finish());

        // Surface how many keys/endpoints are already configured, mirroring the
        // app's `ApiKeyManager` so the state is visible without reopening a modal.
        if let Some(status) = self.render_byok_status(appearance) {
            content = content.with_child(Container::new(status).with_margin_top(16.).finish());
        }

        Self::render_card_chrome(
            appearance,
            is_selected,
            self.byok_mouse_state.clone(),
            AiAccessSlideAction::SelectByok,
            content.finish(),
        )
    }

    /// "N keys connected" / "1 key and 1 endpoint connected" summary for the
    /// BYOK card, or `None` when nothing is configured yet.
    fn byok_status_text(&self) -> Option<String> {
        fn count_label(count: usize, noun: &str) -> String {
            format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
        }
        match (self.byok_key_count, self.byok_endpoint_count) {
            (0, 0) => None,
            (keys, 0) => Some(format!("{} connected", count_label(keys, "key"))),
            (0, endpoints) => Some(format!("{} connected", count_label(endpoints, "endpoint"))),
            (keys, endpoints) => Some(format!(
                "{} and {} connected",
                count_label(keys, "key"),
                count_label(endpoints, "endpoint"),
            )),
        }
    }

    fn render_byok_status(&self, appearance: &Appearance) -> Option<Box<dyn Element>> {
        const ICON_SIZE: f32 = 14.;

        let text = self.byok_status_text()?;
        let green = appearance.theme().ansi_fg_green();

        let icon = ConstrainedBox::new(Box::new(
            Icon::CheckSkinny.to_warpui_icon(Fill::Solid(green)),
        ))
        .with_width(ICON_SIZE)
        .with_height(ICON_SIZE)
        .finish();

        let label = appearance
            .ui_builder()
            .span(text)
            .with_style(UiComponentStyles {
                font_color: Some(green),
                font_size: Some(14.),
                ..Default::default()
            })
            .build()
            .finish();

        Some(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(icon)
                .with_child(Container::new(label).with_margin_left(8.).finish())
                .finish(),
        )
    }

    fn render_bottom_nav(&self, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
        let back_button = self.back_button.render(
            appearance,
            button::Params {
                content: button::Content::Label("Back".into()),
                theme: &button::themes::Naked,
                options: button::Options {
                    on_click: Some(Box::new(|ctx, _app, _pos| {
                        ctx.dispatch_typed_action(AiAccessSlideAction::BackClicked);
                    })),
                    ..button::Options::default(appearance)
                },
            },
        );

        let no_ai_keystroke = Keystroke::parse("cmdorctrl-enter").unwrap_or_default();
        let no_ai_button = self.no_ai_button.render(
            appearance,
            button::Params {
                content: button::Content::Label("I don't want AI".into()),
                theme: &button::themes::Naked,
                options: button::Options {
                    keystroke: Some(no_ai_keystroke),
                    on_click: Some(Box::new(|ctx, _app, _pos| {
                        ctx.dispatch_typed_action(AiAccessSlideAction::NoAiClicked);
                    })),
                    ..button::Options::default(appearance)
                },
            },
        );

        let can_advance = self.can_advance();
        let enter = Keystroke::parse("enter").unwrap_or_default();
        let next_button = self.next_button.render(
            appearance,
            button::Params {
                content: button::Content::Label("Next".into()),
                theme: &button::themes::Primary,
                options: button::Options {
                    disabled: !can_advance,
                    // Explain why the user can't continue yet.
                    tooltip: (!can_advance).then(|| button::Tooltip {
                        params: tooltip::Params {
                            label: "Add a provider key or custom endpoint to continue".into(),
                            options: tooltip::Options {
                                keyboard_shortcut: None,
                            },
                        },
                        alignment: button::TooltipAlignment::Right,
                    }),
                    keystroke: can_advance.then_some(enter),
                    on_click: Some(Box::new(|ctx, _app, _pos| {
                        ctx.dispatch_typed_action(AiAccessSlideAction::NextClicked);
                    })),
                    ..button::Options::default(appearance)
                },
            },
        );

        let right_buttons = Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(no_ai_button)
            .with_child(Container::new(next_button).with_margin_left(8.).finish())
            .finish();

        let (step_index, step_count) = self.onboarding_state.as_ref(app).progress();
        bottom_nav::onboarding_bottom_nav(
            appearance,
            step_index,
            step_count,
            Some(back_button),
            Some(right_buttons),
        )
    }

    fn render_visual(&self) -> Box<dyn Element> {
        layout::onboarding_right_panel_with_bg(
            Self::VISUAL_IMAGE_PATHS[0],
            layout::FOREGROUND_LAYOUT_DEFAULT,
        )
    }
}

impl Entity for AiAccessSlide {
    type Event = AiAccessSlideEvent;
}

impl View for AiAccessSlide {
    fn ui_name() -> &'static str {
        "AiAccessSlide"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let choice = self.choice(app);

        layout::static_left(
            || self.render_content(appearance, choice, app),
            || self.render_visual(),
        )
    }
}

impl AiAccessSlide {
    fn select_choice(&mut self, choice: AiAccessChoice, ctx: &mut ViewContext<Self>) {
        self.onboarding_state.update(ctx, |model, ctx| {
            model.set_ai_access_choice(choice, ctx);
        });
        ctx.notify();
    }

    fn next(&mut self, ctx: &mut ViewContext<Self>) {
        self.onboarding_state.update(ctx, |model, ctx| {
            model.next(ctx);
        });
    }

    pub(crate) fn set_byok_status(
        &mut self,
        key_count: usize,
        endpoint_count: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.byok_key_count == key_count && self.byok_endpoint_count == endpoint_count {
            return;
        }
        self.byok_key_count = key_count;
        self.byok_endpoint_count = endpoint_count;
        ctx.notify();
    }
}

impl OnboardingSlide for AiAccessSlide {
    fn on_up(&mut self, ctx: &mut ViewContext<Self>) {
        self.select_choice(AiAccessChoice::Byok, ctx);
    }

    fn on_down(&mut self, ctx: &mut ViewContext<Self>) {
        self.select_choice(AiAccessChoice::Byok, ctx);
    }

    fn on_enter(&mut self, ctx: &mut ViewContext<Self>) {
        if self.can_advance() {
            self.next(ctx);
        }
    }

    fn on_cmd_or_ctrl_enter(&mut self, ctx: &mut ViewContext<Self>) {
        self.onboarding_state.update(ctx, |model, ctx| {
            model.request_no_ai_confirmation(NoAiConfirmationSource::AiAccess, ctx);
        });
    }
}

impl TypedActionView for AiAccessSlide {
    type Action = AiAccessSlideAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            AiAccessSlideAction::SelectByok => {
                self.select_choice(AiAccessChoice::Byok, ctx);
            }
            AiAccessSlideAction::AddApiKeyClicked => {
                self.select_choice(AiAccessChoice::Byok, ctx);
                ctx.emit(AiAccessSlideEvent::AddApiKeyRequested);
            }
            AiAccessSlideAction::AddCustomEndpointClicked => {
                self.select_choice(AiAccessChoice::Byok, ctx);
                ctx.emit(AiAccessSlideEvent::AddCustomEndpointRequested);
            }
            AiAccessSlideAction::BackClicked => {
                self.onboarding_state.update(ctx, |model, ctx| {
                    model.back(ctx);
                });
            }
            AiAccessSlideAction::NextClicked => {
                if self.can_advance() {
                    self.next(ctx);
                }
            }
            AiAccessSlideAction::NoAiClicked => {
                self.onboarding_state.update(ctx, |model, ctx| {
                    model.request_no_ai_confirmation(NoAiConfirmationSource::AiAccess, ctx);
                });
            }
        }
    }
}
