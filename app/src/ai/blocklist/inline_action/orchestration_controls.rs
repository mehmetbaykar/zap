//! Shared orchestration controls used by the plan-card
//! `OrchestrationConfigBlockView` to edit the run-wide harness / model /
//! fields of an `OrchestrationConfig`.
//!
//! The generic parameter `A` is the parent view's typed action, letting
//! `OrchestrationConfigBlockView` implement [`OrchestrationControlAction`]
//! to map field-change events to its own action enum.
//!
//! Cloud-only data sources and controls are intentionally absent from
//! this fork. Orchestration here selects only a local harness and model.

use std::collections::HashMap;

use ai::agent::orchestration_config::{OrchestrationConfig, OrchestrationExecutionMode};
use pathfinder_color::ColorU;
use pathfinder_geometry::vector::{vec2f, Vector2F};
use warp_cli::agent::Harness;
use warp_core::ui::theme::Fill;
use warpui::elements::{
    ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Empty, Expanded, Flex,
    Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, Point, Radius,
    Text,
};
use warpui::event::DispatchedEvent;
use warpui::platform::Cursor;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponentStyles};
use warpui::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext,
    SingletonEntity, SizeConstraint, View, ViewContext, ViewHandle,
};

use warp_core::features::FeatureFlag;

use crate::ai::auth_secret_types::auth_secret_types_for_harness;
use crate::ai::blocklist::inline_action::host_picker::HostPicker;
use crate::ai::execution_profiles::model_menu_items::available_model_menu_items;
use crate::ai::harness_display;
use crate::ai::llms::LLMInfo;
use crate::ai::local_harness_setup::{
    local_harness_is_product_enabled, local_harness_product_disabled_message,
    local_harness_setup_state, LocalHarnessSetupState,
};
use crate::appearance::Appearance;
use crate::menu::{MenuItem, MenuItemFields};
use crate::ui_components::blended_colors;
use crate::view_components::dropdown::{
    Dropdown, DropdownAction, DropdownItemAction, DropdownStyle,
};
use crate::view_components::FilterableDropdown;
use crate::LLMPreferences;

// ── Shared constants ────────────────────────────────────────────────

// Zap: `WARP_WORKER_HOST` used to come from the removed cloud
// `connected_self_hosted_workers` module; inlined as a literal since it is only ever
// used as a sentinel meaning "run locally" for `OrchestrationExecutionMode::Local`.
pub const ORCHESTRATION_WARP_WORKER_HOST: &str = "warp";
pub const ORCHESTRATION_ENV_NONE_LABEL: &str = "Empty environment";
pub const ORCHESTRATION_PICKER_HEIGHT: f32 = 36.;
pub const ORCHESTRATION_PICKER_BORDER_WIDTH: f32 = 1.;
pub const ORCHESTRATION_PICKER_FONT_SIZE: f32 = 14.;
pub const ORCHESTRATION_PICKER_RADIUS: f32 = 4.;
pub const ORCHESTRATION_PICKER_MAX_WIDTH: f32 = 205.;

const DEFAULT_MODEL_LABEL: &str = "Default model";
const ORCHESTRATION_SEGMENTED_CONTROL_PADDING: f32 = 4.;
const ORCHESTRATION_SEGMENT_VERTICAL_PADDING: f32 = 4.;
const AUTH_SECRET_INHERIT_LABEL: &str = "Skip (advanced)";
pub const AUTH_SECRET_COLUMN_LABEL: &str = "API key";
const AUTH_SECRET_CREATE_NEW_LABEL: &str = "New API key…";
// ── Action trait ────────────────────────────────────────────────────

/// Trait implemented by the plan-card action so shared picker creation and
/// render helpers can produce the correct action variant.
pub trait OrchestrationControlAction: DropdownItemAction + Clone {
    fn execution_mode_toggled(is_remote: bool) -> Self;
    fn model_changed(model_id: String) -> Self;
    fn harness_changed(harness_type: String) -> Self;
    fn environment_changed(environment_id: String) -> Self;
    fn create_environment_requested() -> Self;
    fn auth_secret_changed(name: Option<String>) -> Self;
    fn create_new_auth_secret_requested() -> Self;
}

// ── Shared edit state ───────────────────────────────────────────────

/// Run-wide configuration fields shared between the confirmation card
/// editor and the plan-card config block. Card-specific fields
/// (agent_run_configs, base_prompt, summary, skills)
/// remain on the per-view state structs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationEditState {
    pub model_id: String,
    pub harness_type: String,
    pub execution_mode: OrchestrationExecutionMode,
    pub auth_secret_name: Option<String>,
}

impl OrchestrationEditState {
    pub(crate) fn sanitize_for_local_execution(&mut self) {
        let Some(harness) = Harness::parse_local_child_harness(&self.harness_type) else {
            return;
        };
        if local_harness_product_disabled_message(harness).is_some() {
            self.harness_type = "oz".to_string();
            self.model_id.clear();
        }
    }
    pub fn from_run_agents_fields(
        model_id: &str,
        harness_type: &str,
        execution_mode: &OrchestrationExecutionMode,
    ) -> Self {
        Self {
            model_id: model_id.to_string(),
            harness_type: harness_type.to_string(),
            execution_mode: execution_mode.clone(),
            auth_secret_name: None,
        }
    }

    pub fn from_orchestration_config(config: &OrchestrationConfig) -> Self {
        let mut state = Self {
            model_id: config.model_id.clone(),
            harness_type: config.harness_type.clone(),
            execution_mode: config.execution_mode.clone(),
            auth_secret_name: None,
        };
        if matches!(state.execution_mode, OrchestrationExecutionMode::Local) {
            state.sanitize_for_local_execution();
        }
        state
    }

    /// Toggle Local ↔ Cloud. Resets OpenCode to Oz when switching
    /// to Cloud (unsupported combination).
    pub fn toggle_execution_mode_to_remote(&mut self, is_remote: bool) {
        if is_remote {
            if self.harness_type.eq_ignore_ascii_case("opencode") {
                self.harness_type = "oz".to_string();
            }
            if !self.execution_mode.is_remote() {
                self.execution_mode = OrchestrationExecutionMode::Remote {
                    environment_id: String::new(),
                    worker_host: ORCHESTRATION_WARP_WORKER_HOST.to_string(),
                };
            }
        } else {
            self.execution_mode = OrchestrationExecutionMode::Local;
            self.sanitize_for_local_execution();
        }
    }

    pub fn set_environment_id(&mut self, environment_id: String) {
        if let OrchestrationExecutionMode::Remote {
            environment_id: id, ..
        } = &mut self.execution_mode
        {
            *id = environment_id;
        }
    }

    pub fn set_worker_host(&mut self, worker_host: String) {
        if let OrchestrationExecutionMode::Remote {
            worker_host: host, ..
        } = &mut self.execution_mode
        {
            *host = worker_host;
        }
    }

    /// Returns `Some(reason)` if Accept / Apply must be disabled.
    /// Hard blocks: OpenCode + Cloud, and product-disabled local harnesses.
    pub fn accept_disabled_reason(&self) -> Option<&'static str> {
        match &self.execution_mode {
            OrchestrationExecutionMode::Local => {
                Harness::parse_local_child_harness(&self.harness_type)
                    .and_then(local_harness_product_disabled_message)
            }
            OrchestrationExecutionMode::Remote { .. }
                if self.harness_type.eq_ignore_ascii_case("opencode") =>
            {
                Some(
                    "OpenCode is not supported on Cloud yet. Switch to Local or pick a different harness.",
                )
            }
            OrchestrationExecutionMode::Remote { .. } => None,
        }
    }

    /// Fills in empty fields from the approved orchestration config.
    /// When the LLM omits harness/model/execution_mode to inherit from
    /// the active config, the raw request arrives with defaults (empty
    /// harness, empty model, Local mode). This resolves those to the
    /// config values so the UI shows the intended settings.
    pub fn resolve_from_config(&mut self, config: &OrchestrationConfig) {
        if self.harness_type.is_empty() && !config.harness_type.is_empty() {
            self.harness_type = config.harness_type.clone();
        }
        if self.model_id.is_empty() && !config.model_id.is_empty() {
            self.model_id = config.model_id.clone();
        }
        if !self.execution_mode.is_remote() && config.execution_mode.is_remote() {
            self.execution_mode = config.execution_mode.clone();
        }
        if matches!(self.execution_mode, OrchestrationExecutionMode::Local) {
            self.sanitize_for_local_execution();
        }
    }

    /// Unconditionally overrides model, harness, and execution mode
    /// from the approved orchestration config. The plan config is the
    /// user-approved source of truth — the LLM's run_agents call may
    /// omit or set these differently, but the config always wins.
    pub fn override_from_approved_config(&mut self, config: &OrchestrationConfig) {
        self.model_id = config.model_id.clone();
        self.harness_type = config.harness_type.clone();
        self.execution_mode = config.execution_mode.clone();
    }

    /// Converts to a native `OrchestrationConfig` for storage / match.
    pub fn to_orchestration_config(&self) -> OrchestrationConfig {
        OrchestrationConfig {
            model_id: self.model_id.clone(),
            harness_type: self.harness_type.clone(),
            execution_mode: self.execution_mode.clone(),
        }
    }
}

// ── Picker handles ──────────────────────────────────────────────────

/// Picker view handles shared between card editor and plan-card config
/// block. Generic over the action type `A`.
#[derive(Clone)]
pub struct OrchestrationPickerHandles<A: OrchestrationControlAction> {
    pub model_picker: Option<ViewHandle<FilterableDropdown<A>>>,
    pub harness_picker: Option<ViewHandle<Dropdown<A>>>,
    pub environment_picker: Option<ViewHandle<FilterableDropdown<A>>>,
    pub host_picker: Option<ViewHandle<HostPicker>>,
    pub auth_secret_picker: Option<ViewHandle<Dropdown<A>>>,
    pub local_toggle: MouseStateHandle,
    pub cloud_toggle: MouseStateHandle,
}

impl<A: OrchestrationControlAction> Default for OrchestrationPickerHandles<A> {
    fn default() -> Self {
        Self {
            model_picker: None,
            harness_picker: None,
            environment_picker: None,
            host_picker: None,
            auth_secret_picker: None,
            local_toggle: MouseStateHandle::default(),
            cloud_toggle: MouseStateHandle::default(),
        }
    }
}

// ── Picker styling ──────────────────────────────────────────────────

/// Constructs the shared `UiComponentStyles` for orchestration pickers.
pub fn picker_styles(appearance: &Appearance) -> (UiComponentStyles, PickerColors) {
    let theme = appearance.theme();
    let padding = Coords {
        top: 8.,
        bottom: 8.,
        left: 12.,
        right: 12.,
    };
    let corner_radius = CornerRadius::with_all(Radius::Pixels(ORCHESTRATION_PICKER_RADIUS));
    // The picker bg is a translucent overlay (surface_overlay_1 =
    // fg at 5%). It must stay translucent so that the accent-tinted
    // card background in the config block shows through, and so that
    // gradient-background themes render correctly.
    let background_fill: Fill = theme.surface_overlay_1();
    let background: warpui::elements::Fill = background_fill.into();
    // Border and font colors are intentionally left to the dropdown's
    // default ButtonVariant::Secondary styling, which uses
    // theme.outline() and theme.main_text_color() — both are
    // contrast-aware and adapt correctly to all themes.

    let styles = UiComponentStyles {
        height: Some(ORCHESTRATION_PICKER_HEIGHT),
        background: Some(background),
        border_width: Some(ORCHESTRATION_PICKER_BORDER_WIDTH),
        border_radius: Some(corner_radius),
        font_size: Some(ORCHESTRATION_PICKER_FONT_SIZE),
        padding: Some(padding),
        ..Default::default()
    };
    let colors = PickerColors {
        padding,
        corner_radius,
        background,
    };
    (styles, colors)
}

#[derive(Clone)]
pub struct PickerColors {
    pub padding: Coords,
    pub corner_radius: CornerRadius,
    pub background: warpui::elements::Fill,
}

// ── Picker creation (generic over action type) ──────────────────────

/// Creates a standard dropdown with the shared orchestration picker
/// chrome (border, radius, background, font).
pub fn new_standard_picker_dropdown<A: OrchestrationControlAction, V: View>(
    colors: &PickerColors,
    ctx: &mut ViewContext<V>,
) -> ViewHandle<Dropdown<A>> {
    let padding = colors.padding;
    let corner_radius = colors.corner_radius;
    let background = colors.background;
    ctx.add_typed_action_view(move |ctx_dropdown| {
        let mut dropdown = Dropdown::<A>::new(ctx_dropdown);
        dropdown.set_use_overlay_layer(false, ctx_dropdown);
        dropdown.set_match_menu_width_to_top_bar(true, ctx_dropdown);
        dropdown.set_main_axis_size(MainAxisSize::Max, ctx_dropdown);
        dropdown.set_style(DropdownStyle::ActionButtonSecondary, ctx_dropdown);
        dropdown.set_top_bar_height(ORCHESTRATION_PICKER_HEIGHT, ctx_dropdown);
        dropdown.set_top_bar_max_width(f32::INFINITY);
        dropdown.set_padding(padding, ctx_dropdown);
        dropdown.set_border_radius(corner_radius, ctx_dropdown);
        dropdown.set_background(background, ctx_dropdown);
        dropdown.set_border_width(ORCHESTRATION_PICKER_BORDER_WIDTH, ctx_dropdown);
        dropdown.set_font_size(ORCHESTRATION_PICKER_FONT_SIZE, ctx_dropdown);
        dropdown
    })
}

/// Creates a searchable dropdown with the shared orchestration picker
/// chrome (border, radius, background, font).
pub fn new_standard_filterable_picker_dropdown<A: OrchestrationControlAction, V: View>(
    styles: &UiComponentStyles,
    ctx: &mut ViewContext<V>,
) -> ViewHandle<FilterableDropdown<A>> {
    let styles = *styles;
    ctx.add_typed_action_view(move |ctx_dropdown| {
        let mut dropdown = FilterableDropdown::<A>::new(ctx_dropdown);
        dropdown.set_use_overlay_layer(false, ctx_dropdown);
        dropdown.set_match_menu_width_to_top_bar(true, ctx_dropdown);
        dropdown.set_main_axis_size(MainAxisSize::Max, ctx_dropdown);
        dropdown.set_button_variant(ButtonVariant::Secondary);
        dropdown.set_style(styles);
        dropdown.set_top_bar_height(ORCHESTRATION_PICKER_HEIGHT, ctx_dropdown);
        dropdown.set_top_bar_max_width(f32::INFINITY);
        dropdown
    })
}

/// Returns Warp base-model choices for orchestration.
fn get_base_model_choices<'a>(
    llm_prefs: &'a LLMPreferences,
    is_local: bool,
) -> impl Iterator<Item = &'a LLMInfo> {
    llm_prefs
        .get_base_llm_choices_for_agent_mode()
        .filter(move |llm| is_local || !crate::ai::agent_providers::llm_id::is_byop(&llm.id))
}
/// Populates the model picker based on the active harness.
///
/// - **Oz / empty**: shows the Warp LLM catalog (existing behavior).
/// - **Any non-Oz harness**: shows only the "Default model" entry. There is
///   no local replacement for the per-harness model catalog that used to
///   come from the (cloud-only) harness-availability service, which was
///   stripped from this fork.
pub fn populate_model_picker_for_harness<A: OrchestrationControlAction, V: View>(
    dropdown: &ViewHandle<FilterableDropdown<A>>,
    initial_model_id: &str,
    harness_type: &str,
    is_local: bool,
    ctx: &mut ViewContext<V>,
) {
    let initial_model_id = initial_model_id.to_string();
    let harness_type = harness_type.to_string();
    dropdown.update(ctx, |dropdown, ctx_dropdown| {
        let harness = Harness::parse_orchestration_harness(&harness_type);
        match harness {
            Some(Harness::Oz) | None => {
                // Oz / unset: direct-provider models are excluded for remote runs.
                // Order auto models before the remaining models.
                let llm_prefs = LLMPreferences::as_ref(ctx_dropdown);
                let (auto_models, rest): (Vec<_>, Vec<_>) =
                    get_base_model_choices(llm_prefs, is_local)
                        .partition(|llm| llm.id.as_str().starts_with("auto"));
                let ordered_choices: Vec<_> = auto_models.into_iter().chain(rest).collect();
                let selected_display_name = ordered_choices
                    .iter()
                    .find(|llm| llm.id.to_string() == initial_model_id)
                    .map(|llm| llm.menu_display_name());
                let items = available_model_menu_items(
                    ordered_choices,
                    move |llm| {
                        DropdownAction::select_action_and_close(A::model_changed(
                            llm.id.to_string(),
                        ))
                    },
                    None,
                    None,
                    false,
                    false,
                    ctx_dropdown,
                );
                dropdown.set_rich_items(items, ctx_dropdown);
                if let Some(name) = &selected_display_name {
                    dropdown.set_selected_by_name(name, ctx_dropdown);
                }
            }
            Some(_harness) => {
                // Non-Oz harness: no per-harness model catalog is available
                // locally, so only the default entry is offered.
                let items = vec![default_model_menu_item::<A>()];
                dropdown.set_rich_items(items, ctx_dropdown);
                dropdown.set_selected_by_name(DEFAULT_MODEL_LABEL, ctx_dropdown);
            }
        }
    });
}

/// Creates a "Default model" menu item that emits an empty model_id.
fn default_model_menu_item<A: OrchestrationControlAction>() -> MenuItem<DropdownAction> {
    MenuItem::Item(
        MenuItemFields::new(DEFAULT_MODEL_LABEL).with_on_select_action(
            DropdownAction::select_action_and_close(A::model_changed(String::new())),
        ),
    )
}

/// Returns whether the given model_id is present in the harness-filtered
/// model choices. Used to detect when a harness change invalidates the
/// current model selection.
pub fn is_model_in_filtered_choices<V: View>(
    model_id: &str,
    harness_type: &str,
    is_local: bool,
    ctx: &mut ViewContext<V>,
) -> bool {
    let harness = Harness::parse_orchestration_harness(harness_type);
    match harness {
        Some(Harness::Oz) | None => {
            let llm_prefs = LLMPreferences::as_ref(ctx);
            get_base_model_choices(llm_prefs, is_local).any(|llm| llm.id.to_string() == model_id)
        }
        // Non-Oz harnesses only ever offer the "Default model" (empty id)
        // entry — see `populate_model_picker_for_harness`.
        Some(_harness) => model_id.is_empty(),
    }
}

/// Returns the default model_id for the given harness.
///
/// For Oz this is the first Warp LLM; for non-Oz harnesses it is an empty
/// string (the "Default model" entry).
pub fn first_filtered_model_id<V: View>(
    harness_type: &str,
    ctx: &mut ViewContext<V>,
) -> Option<String> {
    let harness = Harness::parse_orchestration_harness(harness_type);
    match harness {
        Some(Harness::Oz) | None => {
            let llm_prefs = LLMPreferences::as_ref(ctx);
            llm_prefs
                .get_base_llm_choices_for_agent_mode()
                .next()
                .map(|llm| llm.id.to_string())
        }
        Some(_) => Some(String::new()),
    }
}

fn should_show_harness_picker(state: &OrchestrationEditState) -> bool {
    match state.execution_mode {
        OrchestrationExecutionMode::Local | OrchestrationExecutionMode::Remote { .. } => true,
    }
}

/// Populates the harness picker.
///
/// The cloud-only harness-availability service that used to supply this
/// list (with per-workspace enablement) was stripped from this fork, so
/// the list is enumerated locally: Oz plus the local child harnesses
/// (gated on the same feature flag used elsewhere in this file). Gemini
/// stays excluded — it is not yet supported as a multi-agent harness and
/// causes an infinite "Spawning agents" hang.
pub fn populate_harness_picker<A: OrchestrationControlAction, V: View>(
    dropdown: &ViewHandle<Dropdown<A>>,
    initial_harness: &str,
    is_local: bool,
    ctx: &mut ViewContext<V>,
) {
    let initial_harness = initial_harness.to_string();
    dropdown.update(ctx, |dropdown, ctx_dropdown| {
        let candidates = [
            Harness::Oz,
            Harness::Claude,
            Harness::OpenCode,
            Harness::Codex,
        ];

        // Sort enabled harnesses before disabled ones, preserving
        // relative order within each group. When editing a local run,
        // disabled local-child harnesses are dropped entirely rather
        // than just grayed out.
        let mut sorted: Vec<(Harness, bool, LocalHarnessSetupState)> = candidates
            .into_iter()
            .filter_map(|harness| {
                let local_setup_state = local_harness_setup_state(harness);
                let enabled = harness == Harness::Oz
                    || (FeatureFlag::LocalClaudeCodexChildHarnesses.is_enabled()
                        && local_harness_is_product_enabled(harness)
                        && local_setup_state.is_selectable());
                if is_local && !enabled {
                    return None;
                }
                Some((harness, enabled, local_setup_state))
            })
            .collect();
        sorted.sort_by_key(|(_, enabled, _)| !enabled);

        // Empty string is the wire representation of Oz (see
        // `harness_save_key`); everything else parses normally.
        let target_harness = if initial_harness.is_empty() {
            Some(Harness::Oz)
        } else {
            Harness::parse_orchestration_harness(&initial_harness)
        };

        let mut items: Vec<MenuItem<DropdownAction>> = Vec::new();
        let mut selected_name: Option<String> = None;

        for (harness, enabled, local_setup_state) in sorted {
            let display = harness_display::display_name(harness);
            let mut fields =
                MenuItemFields::new(display).with_icon(harness_display::icon_for(harness));
            if let Some(color) = harness_display::brand_color(harness) {
                fields = fields.with_override_icon_color(Fill::from(color));
            }
            if enabled {
                fields = fields.with_on_select_action(DropdownAction::select_action_and_close(
                    A::harness_changed(harness.to_string()),
                ));
            } else {
                fields = fields.with_disabled(true);
                let tooltip = match local_setup_state {
                    LocalHarnessSetupState::MissingHarness { tooltip } => tooltip,
                    LocalHarnessSetupState::ProductDisabled { message } => message,
                    LocalHarnessSetupState::Ready => "Disabled by feature settings",
                };
                fields = fields.with_tooltip(tooltip);
            }
            if selected_name.is_none() && Some(harness) == target_harness {
                selected_name = Some(display.to_string());
            }
            items.push(MenuItem::Item(fields));
        }
        dropdown.set_rich_items(items, ctx_dropdown);
        if let Some(name) = selected_name {
            dropdown.set_selected_by_name(&name, ctx_dropdown);
        }
    });
}

/// Normalizes a harness_type string for use as a HashMap key in
/// per-harness model memory. Empty string (the wire representation
/// of Oz) is mapped to "oz" so saves and lookups are consistent.
pub fn harness_save_key(harness_type: &str) -> &str {
    if harness_type.is_empty() {
        "oz"
    } else {
        harness_type
    }
}

fn populate_host_picker<V: View>(
    picker: &ViewHandle<HostPicker>,
    initial_host: &str,
    ctx: &mut ViewContext<V>,
) {
    let initial = if initial_host.trim().is_empty() {
        ORCHESTRATION_WARP_WORKER_HOST.to_string()
    } else {
        initial_host.to_string()
    };
    picker.update(ctx, |picker, picker_ctx| {
        picker.set_options(None, None, Vec::new(), picker_ctx);
        picker.set_selected(&initial, picker_ctx);
    });
}

// ── Default environment resolution ──────────────────────────────────

/// No cloud environments remain: the cloud-only environment service and
/// the settings store that used to back this lookup were both stripped
/// from this fork, so there is never a default environment to resolve.
pub fn resolve_default_environment_id(_ctx: &AppContext) -> Option<String> {
    None
}

/// No-op: cloud agent settings (where the environment selection used to
/// be persisted) were stripped along with the rest of the cloud stack.
pub fn persist_environment_selection<V: View>(_environment_id: &str, _ctx: &mut ViewContext<V>) {}

// ── Auth secret helpers ────────────────────────────────────────────

/// Returns `true` when the auth secret picker should be visible: Cloud +
/// non-Oz + a harness with at least one supported auth-secret type. Local
/// non-Oz children inherit auth from the user's shell environment.
pub fn should_show_auth_secret_picker(state: &OrchestrationEditState) -> bool {
    if !state.execution_mode.is_remote() {
        return false;
    }
    let Some(harness) = Harness::parse_orchestration_harness(&state.harness_type) else {
        return false;
    };
    if harness == Harness::Oz {
        return false;
    }
    !auth_secret_types_for_harness(harness).is_empty()
}

/// No cloud agent settings remain (the store this used to read
/// `last_selected_auth_secret` from was stripped along with the rest of
/// the cloud stack), so there is never a persisted default to resolve.
pub fn resolve_default_auth_secret_for_harness(
    _harness_type: &str,
    _ctx: &AppContext,
) -> Option<String> {
    None
}

/// Populates the auth secret picker for the given harness. Only the
/// "Inherit key from environment" entry is ever available: managed
/// secrets used to be loaded from the (cloud-only) harness-availability
/// service, which was stripped from this fork, so there is no longer a
/// managed-secret catalog to list or lazily fetch.
pub fn populate_auth_secret_picker_for_harness<A: OrchestrationControlAction, V: View>(
    dropdown: &ViewHandle<Dropdown<A>>,
    _initial_secret_name: Option<&str>,
    harness_type: &str,
    ctx: &mut ViewContext<V>,
) {
    let Some(harness) = Harness::parse_orchestration_harness(harness_type) else {
        return;
    };
    if harness == Harness::Oz {
        return;
    }
    dropdown.update(ctx, |dropdown, ctx_dropdown| {
        let items = vec![MenuItem::Item(
            MenuItemFields::new(AUTH_SECRET_INHERIT_LABEL).with_on_select_action(
                DropdownAction::select_action_and_close(A::auth_secret_changed(None)),
            ),
        )];
        dropdown.set_rich_items(items, ctx_dropdown);
        dropdown.set_selected_by_name(AUTH_SECRET_INHERIT_LABEL, ctx_dropdown);
    });
}

/// Updates the edit state with a new auth secret selection.
///
/// Does NOT persist the selection: it used to be written to
/// `CloudAgentSettings.last_selected_auth_secret` so it would survive
/// across sessions and stay in sync with cloud mode's single-agent
/// picker, but that settings store was stripped along with the rest of
/// the cloud stack, so the selection is UI-session-only now.
///
/// Does NOT repopulate the picker — doing so from inside the action the
/// picker just dispatched would re-enter the dropdown's view and trip
/// warpui's circular-update guard. The dropdown already reflects the
/// chosen value.
pub fn apply_auth_secret_change<A: OrchestrationControlAction, V: View>(
    state: &mut OrchestrationEditState,
    _handles: &OrchestrationPickerHandles<A>,
    new_name: Option<String>,
    _ctx: &mut ViewContext<V>,
) {
    state.auth_secret_name = new_name.filter(|s| !s.trim().is_empty());
}

// ── Shared action helpers ───────────────────────────────────────────

/// Handles a harness change for both card views: saves/restores per-harness
/// model selection, repopulates the model picker, and re-resolves the auth
/// secret selection for the new harness.
///
/// Does NOT re-enter the harness picker that dispatched this action.
pub fn apply_harness_change<A: OrchestrationControlAction, V: View>(
    state: &mut OrchestrationEditState,
    memory: &mut HashMap<String, String>,
    handles: &OrchestrationPickerHandles<A>,
    new_harness_type: &str,
    fallback_base_model_id: impl FnOnce(&mut ViewContext<V>) -> Option<String>,
    ctx: &mut ViewContext<V>,
) {
    // Save current model for the old harness.
    let old_key = harness_save_key(&state.harness_type).to_string();
    memory.insert(old_key, state.model_id.clone());
    state.harness_type = new_harness_type.to_string();

    let is_local = !state.execution_mode.is_remote();
    if is_local {
        state.sanitize_for_local_execution();
        if state.harness_type != new_harness_type {
            if let Some(handle) = &handles.harness_picker {
                populate_harness_picker(handle, &state.harness_type, true, ctx);
            }
        }
    }
    // Try to restore a previously saved model for this harness.
    let new_key = harness_save_key(&state.harness_type);
    let restored = memory
        .get(new_key)
        .filter(|id| is_model_in_filtered_choices(id, &state.harness_type, is_local, ctx))
        .cloned();
    if let Some(saved_id) = restored {
        state.model_id = saved_id;
    } else if !is_model_in_filtered_choices(&state.model_id, &state.harness_type, is_local, ctx) {
        // No saved model — fall back to conversation base model
        // for Oz, or default for non-Oz.
        let reset_id = fallback_base_model_id(ctx)
            .filter(|id| is_model_in_filtered_choices(id, &state.harness_type, is_local, ctx))
            .or_else(|| first_filtered_model_id(&state.harness_type, ctx))
            .unwrap_or_default();
        state.model_id = reset_id;
    }
    if let Some(handle) = &handles.model_picker {
        populate_model_picker_for_harness(
            handle,
            &state.model_id,
            &state.harness_type,
            is_local,
            ctx,
        );
    }

    // Re-resolve auth secret from settings for the new harness.
    state.auth_secret_name = resolve_default_auth_secret_for_harness(new_harness_type, ctx);
    if let Some(handle) = &handles.auth_secret_picker {
        populate_auth_secret_picker_for_harness(
            handle,
            state.auth_secret_name.as_deref(),
            new_harness_type,
            ctx,
        );
    }
}

/// Handles an execution-mode toggle for both card views: toggles the
/// mode, revalidates/resets the model_id if invalid for the new mode,
/// repopulates the model picker, and syncs all picker selections.
pub fn apply_execution_mode_change<A: OrchestrationControlAction, V: View>(
    state: &mut OrchestrationEditState,
    handles: &OrchestrationPickerHandles<A>,
    is_remote: bool,
    fallback_base_model_id: impl FnOnce(&mut ViewContext<V>) -> Option<String>,
    ctx: &mut ViewContext<V>,
) {
    state.toggle_execution_mode_to_remote(is_remote);
    let is_local = !state.execution_mode.is_remote();
    if let Some(handle) = &handles.harness_picker {
        populate_harness_picker(handle, &state.harness_type, is_local, ctx);
    }
    // Pre-fill environment with the last-selected one when switching to Cloud.
    if is_remote {
        if let OrchestrationExecutionMode::Remote { environment_id, .. } = &state.execution_mode {
            if environment_id.is_empty() {
                if let Some(default_env) = resolve_default_environment_id(ctx) {
                    state.set_environment_id(default_env);
                }
            }
        }
    }
    if !is_model_in_filtered_choices(&state.model_id, &state.harness_type, is_local, ctx) {
        let reset_id = fallback_base_model_id(ctx)
            .filter(|id| is_model_in_filtered_choices(id, &state.harness_type, is_local, ctx))
            .or_else(|| first_filtered_model_id(&state.harness_type, ctx))
            .unwrap_or_default();
        state.model_id = reset_id;
    }
    if let Some(handle) = &handles.model_picker {
        populate_model_picker_for_harness(
            handle,
            &state.model_id,
            &state.harness_type,
            is_local,
            ctx,
        );
    }
    if let Some(handle) = &handles.host_picker {
        let initial_host = match &state.execution_mode {
            OrchestrationExecutionMode::Remote { worker_host, .. } => worker_host.as_str(),
            OrchestrationExecutionMode::Local => ORCHESTRATION_WARP_WORKER_HOST,
        };
        populate_host_picker(handle, initial_host, ctx);
    }
    sync_picker_selections(state, handles, ctx);
}

// ── Picker repopulation + selection sync ──

/// Repopulates the harness, model, and auth-secret pickers from the
/// current server-provided data, revalidates `state.model_id` against
/// the updated catalog (resetting to default if the model disappeared),
/// then re-syncs dropdown selections.
pub fn repopulate_all_pickers<A: OrchestrationControlAction, V: View>(
    state: &mut OrchestrationEditState,
    handles: &OrchestrationPickerHandles<A>,
    ctx: &mut ViewContext<V>,
) {
    let is_local = !state.execution_mode.is_remote();
    if is_local {
        state.sanitize_for_local_execution();
    }
    if let Some(handle) = &handles.harness_picker {
        populate_harness_picker(handle, &state.harness_type, is_local, ctx);
    }
    // Reset model if it disappeared from the harness's catalog.
    if !is_model_in_filtered_choices(&state.model_id, &state.harness_type, is_local, ctx) {
        if let Some(first_id) = first_filtered_model_id(&state.harness_type, ctx) {
            state.model_id = first_id;
        }
    }
    if let Some(handle) = &handles.model_picker {
        populate_model_picker_for_harness(
            handle,
            &state.model_id,
            &state.harness_type,
            is_local,
            ctx,
        );
    }
    // No cloud harness-availability service remains to validate a
    // previously-selected managed secret against, or to re-seed a default
    // from — `resolve_default_auth_secret_for_harness` always returns
    // `None` now (see its doc comment).
    if let Some(handle) = &handles.auth_secret_picker {
        populate_auth_secret_picker_for_harness(
            handle,
            state.auth_secret_name.as_deref(),
            &state.harness_type,
            ctx,
        );
    }
    if let Some(handle) = &handles.host_picker {
        let initial_host = match &state.execution_mode {
            OrchestrationExecutionMode::Remote { worker_host, .. } => worker_host.as_str(),
            OrchestrationExecutionMode::Local => ORCHESTRATION_WARP_WORKER_HOST,
        };
        populate_host_picker(handle, initial_host, ctx);
    }
    sync_picker_selections(state, handles, ctx);
}

pub fn sync_picker_selections<A: OrchestrationControlAction, V: View>(
    state: &OrchestrationEditState,
    handles: &OrchestrationPickerHandles<A>,
    ctx: &mut ViewContext<V>,
) {
    if let Some(model_picker) = handles.model_picker.clone() {
        let target_model_id = state.model_id.clone();
        let harness_type = state.harness_type.clone();
        model_picker.update(ctx, |dropdown, ctx_dropdown| {
            let harness = Harness::parse_orchestration_harness(&harness_type);
            let display_name = match harness {
                Some(Harness::Oz) | None => {
                    let llm_prefs = LLMPreferences::as_ref(ctx_dropdown);
                    llm_prefs
                        .get_base_llm_choices_for_agent_mode()
                        .find(|llm| llm.id.to_string() == target_model_id)
                        .map(|llm| llm.menu_display_name())
                }
                // Non-Oz harnesses only ever offer the "Default model"
                // entry — see `populate_model_picker_for_harness`.
                Some(_harness) => Some(DEFAULT_MODEL_LABEL.to_string()),
            };
            if let Some(name) = &display_name {
                dropdown.set_selected_by_name(name, ctx_dropdown);
            }
        });
    }
    if let Some(harness_picker) = handles.harness_picker.clone() {
        let harness_type = state.harness_type.clone();
        let show_harness_picker = should_show_harness_picker(state);
        harness_picker.update(ctx, |dropdown, ctx_dropdown| {
            if show_harness_picker {
                dropdown.set_enabled(ctx_dropdown);
            } else {
                dropdown.set_disabled(ctx_dropdown);
            }
            let target = Harness::parse_orchestration_harness(&harness_type).unwrap_or(Harness::Oz);
            let display = harness_display::display_name(target).to_string();
            dropdown.set_selected_by_name(&display, ctx_dropdown);
        });
    }
    if let Some(environment_picker) = handles.environment_picker.clone() {
        // No cloud environments remain to look up by id — the picker
        // only ever shows the "(no environment)" entry.
        environment_picker.update(ctx, |dropdown, ctx_dropdown| {
            dropdown.set_selected_by_name(ORCHESTRATION_ENV_NONE_LABEL, ctx_dropdown);
        });
    }
    if let Some(host_picker) = handles.host_picker.clone() {
        let worker_host = match &state.execution_mode {
            OrchestrationExecutionMode::Remote { worker_host, .. } => worker_host.clone(),
            OrchestrationExecutionMode::Local => ORCHESTRATION_WARP_WORKER_HOST.to_string(),
        };
        host_picker.update(ctx, |picker, picker_ctx| {
            picker.set_selected(&worker_host, picker_ctx);
        });
    }
    if let Some(auth_secret_picker) = handles.auth_secret_picker.clone() {
        let selection = state.auth_secret_name.clone();
        let supports_create_new = Harness::parse_orchestration_harness(&state.harness_type)
            .filter(|h| *h != Harness::Oz)
            .map(|h| !auth_secret_types_for_harness(h).is_empty())
            .unwrap_or(false);
        auth_secret_picker.update(ctx, |dropdown, ctx_dropdown| {
            let label = match &selection {
                Some(name) => name.clone(),
                None if supports_create_new => AUTH_SECRET_CREATE_NEW_LABEL.to_string(),
                None => AUTH_SECRET_INHERIT_LABEL.to_string(),
            };
            dropdown.set_selected_by_name(&label, ctx_dropdown);
        });
    }
}

#[cfg(test)]
#[path = "orchestration_controls_tests.rs"]
mod tests;

// ── Adaptive picker layout ──────────────────────────────────────────

/// Lays out children horizontally at a fixed width when they all fit,
/// otherwise stacks them vertically at full available width.
///
/// Switches to vertical when `n * picker_width + (n-1) * spacing` exceeds
/// the available width from the incoming size constraint.
struct AdaptivePickerRow {
    children: Vec<Box<dyn Element>>,
    picker_width: f32,
    spacing: f32,
    is_vertical: bool,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl AdaptivePickerRow {
    fn new(picker_width: f32, spacing: f32) -> Self {
        Self {
            children: Vec::new(),
            picker_width,
            spacing,
            is_vertical: false,
            size: None,
            origin: None,
        }
    }

    fn add_child(&mut self, child: Box<dyn Element>) {
        self.children.push(child);
    }

    fn finish(self) -> Box<dyn Element> {
        Box::new(self)
    }
}

impl Element for AdaptivePickerRow {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let n = self.children.len();
        if n == 0 {
            self.size = Some(Vector2F::zero());
            return Vector2F::zero();
        }

        let total_horizontal =
            self.picker_width * n as f32 + self.spacing * n.saturating_sub(1) as f32;

        self.is_vertical = total_horizontal > constraint.max.x();

        if self.is_vertical {
            let width = constraint.max.x();
            let mut total_height = 0.0f32;
            for (i, child) in self.children.iter_mut().enumerate() {
                if i > 0 {
                    total_height += self.spacing;
                }
                let child_constraint =
                    SizeConstraint::new(vec2f(width, 0.), vec2f(width, f32::INFINITY));
                let child_size = child.layout(child_constraint, ctx, app);
                total_height += child_size.y();
            }
            let size = vec2f(width, total_height);
            self.size = Some(size);
            size
        } else {
            let mut max_height = 0.0f32;
            for child in self.children.iter_mut() {
                let child_constraint = SizeConstraint::new(
                    vec2f(self.picker_width, 0.),
                    vec2f(self.picker_width, f32::INFINITY),
                );
                let child_size = child.layout(child_constraint, ctx, app);
                max_height = max_height.max(child_size.y());
            }
            let size = vec2f(total_horizontal, max_height);
            self.size = Some(size);
            size
        }
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        for child in &mut self.children {
            child.after_layout(ctx, app);
        }
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let mut current = origin;
        if self.is_vertical {
            for (i, child) in self.children.iter_mut().enumerate() {
                if i > 0 {
                    current += vec2f(0., self.spacing);
                }
                child.paint(current, ctx, app);
                if let Some(size) = child.size() {
                    current += vec2f(0., size.y());
                }
            }
        } else {
            for (i, child) in self.children.iter_mut().enumerate() {
                if i > 0 {
                    current += vec2f(self.spacing, 0.);
                }
                child.paint(current, ctx, app);
                let advance = child.size().map_or(self.picker_width, |s| s.x());
                current += vec2f(advance, 0.);
            }
        }
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let mut handled = false;
        for child in &mut self.children {
            handled |= child.dispatch_event(event, ctx, app);
        }
        handled
    }
}

// ── Render helpers ──────────────────────────────────────────────────

pub fn render_mode_toggle<A: OrchestrationControlAction>(
    is_remote: bool,
    handles: &OrchestrationPickerHandles<A>,
    appearance: &Appearance,
    active_segment_bg: Option<Fill>,
    full_width: bool,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let label = Text::new(
        "Agent location".to_string(),
        appearance.ui_font_family(),
        appearance.monospace_font_size() - 1.,
    )
    .with_color(blended_colors::text_disabled(theme, theme.surface_1()))
    .finish();

    let local_segment = render_segment_button::<A>(
        "Local",
        !is_remote,
        A::execution_mode_toggled(false),
        handles.local_toggle.clone(),
        appearance,
        active_segment_bg,
    );
    let cloud_segment = render_segment_button::<A>(
        "Cloud",
        is_remote,
        A::execution_mode_toggled(true),
        handles.cloud_toggle.clone(),
        appearance,
        active_segment_bg,
    );

    let segment_outer_bg = warp_core::ui::theme::color::internal_colors::fg_overlay_2(theme);
    let segments_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_alignment(MainAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1.0, cloud_segment).finish())
        .with_child(Expanded::new(1.0, local_segment).finish())
        .finish();
    let segmented_control = Container::new(segments_row)
        .with_padding_top(ORCHESTRATION_SEGMENTED_CONTROL_PADDING)
        .with_padding_bottom(ORCHESTRATION_SEGMENTED_CONTROL_PADDING)
        .with_padding_left(ORCHESTRATION_SEGMENTED_CONTROL_PADDING)
        .with_padding_right(ORCHESTRATION_SEGMENTED_CONTROL_PADDING)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .with_background(segment_outer_bg)
        .finish();
    let segmented_control =
        ConstrainedBox::new(segmented_control).with_height(ORCHESTRATION_PICKER_HEIGHT);
    let segmented_control = if full_width {
        segmented_control.finish()
    } else {
        segmented_control
            .with_width(ORCHESTRATION_PICKER_MAX_WIDTH)
            .finish()
    };

    let cross_axis = if full_width {
        CrossAxisAlignment::Stretch
    } else {
        CrossAxisAlignment::Start
    };
    Flex::column()
        .with_cross_axis_alignment(cross_axis)
        .with_child(Container::new(label).with_margin_bottom(6.).finish())
        .with_child(segmented_control)
        .finish()
}

fn render_segment_button<A: OrchestrationControlAction>(
    label: &str,
    is_active: bool,
    on_click: A,
    mouse_state: MouseStateHandle,
    appearance: &Appearance,
    active_bg_override: Option<Fill>,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let label_owned = label.to_string();
    let font_family = appearance.ui_font_family();
    let font_size = ORCHESTRATION_PICKER_FONT_SIZE;
    let active_text_color = blended_colors::text_main(theme, theme.surface_1());
    let inactive_text_color = blended_colors::text_disabled(theme, theme.surface_1());
    let segment_active_bg = active_bg_override
        .unwrap_or_else(|| warp_core::ui::theme::color::internal_colors::fg_overlay_4(theme));
    Hoverable::new(mouse_state, move |_| {
        let text = Text::new(label_owned.clone(), font_family, font_size)
            .with_color(if is_active {
                active_text_color
            } else {
                inactive_text_color
            })
            .finish();
        let centered = warpui::elements::Align::new(text).finish();
        let mut container = Container::new(centered)
            .with_vertical_padding(ORCHESTRATION_SEGMENT_VERTICAL_PADDING)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
        if is_active {
            container = container.with_background(segment_active_bg);
        }
        container.finish()
    })
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action(on_click.clone());
    })
    .with_cursor(Cursor::PointingHand)
    .finish()
}

pub fn render_picker_row<A: OrchestrationControlAction>(
    state: &OrchestrationEditState,
    handles: &OrchestrationPickerHandles<A>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    render_picker_row_with_layout(state, handles, appearance, false)
}

/// Renders pickers vertically at full width when `vertical` is true,
/// or in the original horizontal layout when false.
pub fn render_picker_row_with_layout<A: OrchestrationControlAction>(
    state: &OrchestrationEditState,
    handles: &OrchestrationPickerHandles<A>,
    appearance: &Appearance,
    vertical: bool,
) -> Box<dyn Element> {
    let show_harness_picker = should_show_harness_picker(state);

    if vertical {
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.);

        let add = |col: &mut Flex, label: &str, picker: Option<Box<dyn Element>>| {
            col.add_child(render_picker_column(label, picker, appearance));
        };

        if show_harness_picker {
            add(
                &mut column,
                "Agent harness",
                handles
                    .harness_picker
                    .as_ref()
                    .map(|p| ChildView::new(p).finish()),
            );
        }
        add(
            &mut column,
            "Base model",
            handles
                .model_picker
                .as_ref()
                .map(|p| ChildView::new(p).finish()),
        );

        Container::new(column.finish())
            .with_margin_top(12.)
            .finish()
    } else {
        let mut row = AdaptivePickerRow::new(ORCHESTRATION_PICKER_MAX_WIDTH, 12.);

        let add_picker =
            |row: &mut AdaptivePickerRow, label: &str, picker: Option<Box<dyn Element>>| {
                let col = render_picker_column(label, picker, appearance);
                row.add_child(col);
            };

        if show_harness_picker {
            add_picker(
                &mut row,
                "Agent harness",
                handles
                    .harness_picker
                    .as_ref()
                    .map(|p| ChildView::new(p).finish()),
            );
        }
        add_picker(
            &mut row,
            "Base model",
            handles
                .model_picker
                .as_ref()
                .map(|p| ChildView::new(p).finish()),
        );
        Container::new(row.finish()).with_margin_top(12.).finish()
    }
}

pub fn render_picker_column(
    label: &str,
    picker: Option<Box<dyn Element>>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let label_el = Text::new(
        label.to_string(),
        appearance.ui_font_family(),
        appearance.monospace_font_size() - 1.,
    )
    .with_color(blended_colors::text_disabled(theme, theme.surface_1()))
    .finish();

    let body: Box<dyn Element> = picker.unwrap_or_else(|| Empty::new().finish());
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(label_el)
        .with_child(body)
        .finish()
}

pub fn render_validation_error(
    reason: impl Into<String>,
    color: ColorU,
    appearance: &Appearance,
) -> Box<dyn Element> {
    Container::new(
        Text::new(
            reason.into(),
            appearance.ui_font_family(),
            appearance.monospace_font_size() - 1.,
        )
        .with_color(color)
        .finish(),
    )
    .with_margin_bottom(8.)
    .finish()
}

/// No cloud environments remain (the cloud-only environment service was
/// stripped from this fork), so there is nothing left to recommend here.
pub fn empty_env_recommendation_message(
    _execution_mode: &OrchestrationExecutionMode,
    _app: &AppContext,
) -> Option<String> {
    None
}
