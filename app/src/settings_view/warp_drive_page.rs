use warp_core::features::FeatureFlag;
use warp_core::settings::ToggleableSetting as _;
use warp_errors::report_if_error;
use warpui::elements::{Element, MouseStateHandle};
use warpui::keymap::ContextPredicate;
use warpui::ui_components::{components::UiComponent, switch::SwitchStateHandle};
use warpui::{
    id, Action, AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
};

use super::settings_page::{
    render_body_item, AdditionalInfo, MatchData, PageType, SettingsPageMeta,
    SettingsPageViewHandle, SettingsWidget,
};
use super::{
    flags, LocalOnlyIconState, SettingActionPairContexts, SettingActionPairDescriptions,
    SettingsAction, SettingsSection, ToggleSettingActionPair, ToggleState,
};
use crate::appearance::Appearance;
use crate::drive::settings::WarpDriveSettings;

#[derive(Debug, Clone)]
pub enum WarpDriveSettingsPageAction {
    ToggleShowWarpDrive,
    OpenUrl(String),
}

pub fn init_actions_from_parent_view<T: Action + Clone>(
    app: &mut AppContext,
    context: &ContextPredicate,
    builder: fn(SettingsAction) -> T,
) {
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::custom(
            SettingActionPairDescriptions::new("Enable Zap Drive", "Disable Zap Drive"),
            builder(SettingsAction::ZapDrive(
                WarpDriveSettingsPageAction::ToggleShowWarpDrive,
            )),
            SettingActionPairContexts::new(
                context.clone() & !id!(flags::ENABLE_WARP_DRIVE),
                context.clone() & id!(flags::ENABLE_WARP_DRIVE),
            ),
            None,
        )
        .with_enabled(|| FeatureFlag::ZapNewSettingsModes.is_enabled())],
        app,
    );
}

pub struct WarpDriveSettingsPageView {
    page: PageType<Self>,
}

impl WarpDriveSettingsPageView {
    pub fn new(_ctx: &mut ViewContext<Self>) -> Self {
        Self {
            page: PageType::new_uncategorized(
                vec![Box::new(WarpDriveToggleWidget::default())],
                None,
            ),
        }
    }
}

impl Entity for WarpDriveSettingsPageView {
    type Event = ();
}

impl TypedActionView for WarpDriveSettingsPageView {
    type Action = WarpDriveSettingsPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            WarpDriveSettingsPageAction::ToggleShowWarpDrive => {
                WarpDriveSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.enable_warp_drive.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            WarpDriveSettingsPageAction::OpenUrl(url) => {
                ctx.open_url(url.as_str());
            }
        }
    }
}

impl View for WarpDriveSettingsPageView {
    fn ui_name() -> &'static str {
        "WarpDrivePage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

impl SettingsPageMeta for WarpDriveSettingsPageView {
    fn section() -> SettingsSection {
        SettingsSection::ZapDrive
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        FeatureFlag::ZapNewSettingsModes.is_enabled()
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<WarpDriveSettingsPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<WarpDriveSettingsPageView>) -> Self {
        SettingsPageViewHandle::ZapDrive(view_handle)
    }
}

#[derive(Default)]
struct WarpDriveToggleWidget {
    switch_state: SwitchStateHandle,
    info_icon_mouse_state: MouseStateHandle,
}

impl SettingsWidget for WarpDriveToggleWidget {
    type View = WarpDriveSettingsPageView;

    fn search_terms(&self) -> &str {
        "zap drive tools panel command palette search workflows prompts notebooks environment variables"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let settings = WarpDriveSettings::as_ref(app);

        render_body_item::<WarpDriveSettingsPageAction>(
            "Zap Drive".into(),
            Some(AdditionalInfo {
                mouse_state: self.info_icon_mouse_state.clone(),
                on_click_action: Some(WarpDriveSettingsPageAction::OpenUrl(
                    "".to_string(),
                )),
                secondary_text: None,
                tooltip_override_text: None,
            }),
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*settings.enable_warp_drive)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(WarpDriveSettingsPageAction::ToggleShowWarpDrive);
                })
                .finish(),
            Some("Zap Drive is a local workspace in your terminal where you can save Workflows, Notebooks, Prompts, and Environment Variables on this device.".into()),
        )
    }
}
