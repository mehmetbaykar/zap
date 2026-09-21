//! This module contains common utilities for rendering Blocklist AI UI.
use std::sync::LazyLock;

use pathfinder_color::ColorU;
use pathfinder_geometry::vector::vec2f;
use thousands::Separable;
use warp_core::features::FeatureFlag;
use warp_core::ui::appearance::Appearance;
use warpui::elements::{
    ChildAnchor, ConstrainedBox, Container, CrossAxisAlignment, Flex, Hoverable, MainAxisAlignment,
    MainAxisSize, MouseStateHandle, OffsetPositioning, ParentAnchor, ParentElement,
    ParentOffsetBounds, Stack,
};
use warpui::fonts::Weight;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::ui_components::text::Span;
use warpui::{AppContext, Element, EntityId, EventContext, SingletonEntity};

use crate::themes::theme::{AnsiColorIdentifier, Fill, WarpTheme};
use crate::ui_components::icons::Icon;

const PROVIDER_BUTTON_ICON_SIZE: f32 = 14.;
const PROVIDER_BUTTON_ICON_TEXT_GAP: f32 = 8.;

/// Text to use as a label throughout the app for user interactions that will attach selected
/// block(s) or text selections to a new AI query.
pub static ATTACH_AS_AGENT_MODE_CONTEXT_TEXT: LazyLock<&'static str> =
    LazyLock::new(|| Box::leak(crate::t!("menu-attach-as-agent-context").into_boxed_str()));

/// Claude/Anthropic brand color (official brand orange #D97757).
/// Reference: https://github.com/anthropics/skills/blob/main/skills/brand-guidelines/SKILL.md
pub const CLAUDE_ORANGE: ColorU = ColorU {
    r: 0xD9,
    g: 0x77,
    b: 0x57,
    a: 0xFF,
};

/// Returns the color to be used for various AI signifiers
/// input with AI mode).
pub fn ai_brand_color(theme: &WarpTheme) -> ColorU {
    AnsiColorIdentifier::Magenta
        .to_ansi_color(&theme.terminal_colors().normal)
        .into()
}

/// Returns the color to be used for error UI throughout Agent Mode (like the "request limit
/// exceeded" chip).
pub fn error_color(theme: &WarpTheme) -> ColorU {
    AnsiColorIdentifier::Red
        .to_ansi_color(&theme.terminal_colors().normal)
        .into()
}

/// Returns the AI icon element to be rendered in AI output blocks and the terminal input when in
/// AI mode. Takes a color parameter as the solid fill for the icon. We use [ai_brand_color] in most
/// cases.
pub fn render_ai_agent_mode_icon(app: &AppContext, color: impl Into<Fill>) -> Box<dyn Element> {
    render_input_icon(Icon::AgentMode, color.into(), app)
}

/// Returns the icon element to be rendered in the terminal input when
/// the user is making a follow up AI query in an existing conversation. Takes a color parameter as the solid fill for the icon.
pub fn render_ai_follow_up_icon(
    mouse_state: MouseStateHandle,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    Hoverable::new(mouse_state, |state| {
        let mut stack = Stack::new().with_child(render_input_icon(
            Icon::CornerRight,
            appearance.theme().foreground(),
            app,
        ));
        if state.is_hovered() {
            let tooltip_background = appearance.theme().tooltip_background();
            let tool_tip = appearance
                .ui_builder()
                .tool_tip(crate::t!("ai-block-follow-up-existing-conversation"))
                .with_style(UiComponentStyles {
                    font_size: Some(12.),
                    background: Some(warpui::elements::Fill::Solid(tooltip_background)),
                    font_color: Some(appearance.theme().background().into_solid()),
                    ..Default::default()
                });
            stack.add_positioned_overlay_child(
                tool_tip.build().finish(),
                OffsetPositioning::offset_from_parent(
                    vec2f(0., -4.),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::BottomLeft,
                ),
            );
        }
        stack.finish()
    })
    .finish()
}

fn render_input_icon(icon: Icon, color: Fill, app: &AppContext) -> Box<dyn Element> {
    // Since the icon is rendered next to monospace text content, its size should scale to
    // based on the current font size -- specifically, its height must match the editor text line
    // height.
    let icon_size = ai_indicator_height(app);
    ConstrainedBox::new(
        Container::new(icon.to_warpui_icon(color).finish())
            .with_uniform_padding(icon_size / 8.)
            .finish(),
    )
    .with_width(icon_size)
    .with_height(icon_size)
    .finish()
}

/// Returns the size to be used for the AI icon in AI output blocks and the terminal input when in
/// AI mode.
///
/// This size is computed based on the user's current font size and line height ratio, such that the
/// size of the icon matches the user's text line height.  This is necessary because the AI icon in
/// the input is rendered next to text in the editor.
pub fn ai_indicator_height(app: &AppContext) -> f32 {
    let appearance = Appearance::as_ref(app);
    app.font_cache().line_height(
        appearance.monospace_font_size(),
        appearance.line_height_ratio(),
    )
}

/// Returns the saved position ID of the attached blocks chip inside the [`AIBlock`] header.
pub fn get_attached_blocks_chip_element_position_id(view_id: EntityId) -> String {
    format!("aiblock:{view_id}.attached_block_chip_position")
}

/// Returns the saved position ID of the overflow menu inside the [`AIBlock`] header.
pub fn get_ai_block_overflow_menu_element_position_id(view_id: EntityId) -> String {
    format!("aiblock:{view_id}.overflow_menu_position")
}

/// Formats credit count to display as whole numbers when the value is effectively a whole number,
/// otherwise displays with one decimal place. A non-zero amount below the displayed precision is
/// shown as `<0.1 credits` rather than rounding to zero, which would read as no cost.
/// Returns a formatted string with proper pluralization ("credit" vs "credits").
pub fn format_credits(credits: f32) -> String {
    if credits > 0.0 && credits < 0.1 {
        return "<0.1 credits".to_string();
    }
    // If the first part of the decimal is 0, we just display the whole number.
    if credits.fract() < 0.1 {
        let whole = credits.trunc() as i32;
        if whole == 1 {
            format!("{whole} credit")
        } else {
            format!("{whole} credits")
        }
    } else {
        format!("{credits:.1} credits")
    }
}

/// Formats a US-cent amount as dollars without rounding a positive charge down to zero.
pub fn format_dollars(cost_in_cents: f32) -> String {
    // Accumulated costs can produce negative zero, which would otherwise render as `$-0.00`.
    let cost_in_cents = if cost_in_cents == 0.0 {
        0.0
    } else {
        cost_in_cents
    };
    let dollars = cost_in_cents / 100.0;
    if cost_in_cents > 0.0 && dollars < 0.01 {
        "<$0.01".to_string()
    } else {
        format!("${dollars:.2}")
    }
}

fn effective_usage_unit(unit: UsageDisplayUnit, cost_in_cents: Option<f32>) -> UsageDisplayUnit {
    if !FeatureFlag::PricingTransparency.is_enabled() {
        return UsageDisplayUnit::Credits;
    }
    match unit {
        UsageDisplayUnit::Credits => UsageDisplayUnit::Credits,
        UsageDisplayUnit::Dollars if cost_in_cents.is_some() => UsageDisplayUnit::Dollars,
        UsageDisplayUnit::Dollars => UsageDisplayUnit::Credits,
    }
}

fn format_usage_unit_value(
    credits: f32,
    cost_in_cents: Option<f32>,
    unit: UsageDisplayUnit,
) -> String {
    match unit {
        UsageDisplayUnit::Credits => format_credits(credits),
        UsageDisplayUnit::Dollars => cost_in_cents
            .map(format_dollars)
            .unwrap_or_else(|| format_credits(credits)),
    }
}

/// Formats a credit count together with its total token count and real
/// dollar cost, e.g. `"20 credits (12,345 tokens, $0.36)"`. See
/// [`format_usage_parenthetical`] for how the parenthetical is built and
/// when it's omitted (including always, when `FeatureFlag::PricingTransparency`
/// is disabled).
pub fn format_credits_with_cost(
    credits: f32,
    tokens: Option<u32>,
    cost_in_cents: Option<f32>,
) -> String {
    let credits_text = format_credits(credits);
    let Some(parenthetical) = format_usage_parenthetical(tokens, cost_in_cents) else {
        return credits_text;
    };
    format!("{credits_text} ({parenthetical})")
}

/// Renders a secondary button with an MCP/skill provider icon and a text label.
pub(crate) fn render_provider_icon_button<F>(
    button_label: &str,
    button_handle: MouseStateHandle,
    appearance: &Appearance,
    icon: Icon,
    color: Fill,
    on_click: F,
) -> Box<dyn Element>
where
    F: FnMut(&mut EventContext) + 'static,
{
    let theme = appearance.theme();
    let font_color = theme.foreground().into_solid();
    let mut label_children = vec![
        ConstrainedBox::new(icon.to_warpui_icon(color).finish())
            .with_width(PROVIDER_BUTTON_ICON_SIZE)
            .with_height(PROVIDER_BUTTON_ICON_SIZE)
            .finish(),
    ];
    label_children.push(
        Container::new(
            Span::new(
                button_label.to_string(),
                UiComponentStyles {
                    font_family_id: Some(appearance.ui_font_family()),
                    font_size: Some(appearance.ui_font_size()),
                    font_weight: Some(Weight::Semibold),
                    font_color: Some(font_color),
                    ..Default::default()
                },
            )
            .build()
            .finish(),
        )
        .with_padding_left(PROVIDER_BUTTON_ICON_TEXT_GAP)
        .finish(),
    );
    let label = Flex::row()
        .with_children(label_children)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_alignment(MainAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min)
        .finish();
    let mut on_click = on_click;
    appearance
        .ui_builder()
        .button(ButtonVariant::Secondary, button_handle)
        .with_custom_label(label)
        .with_style(UiComponentStyles {
            font_weight: Some(Weight::Semibold),
            ..Default::default()
        })
        .build()
        .on_click(move |ctx, _, _| {
            on_click(ctx);
        })
        .finish()
}

#[cfg(test)]
#[path = "view_util_tests.rs"]
mod tests;
