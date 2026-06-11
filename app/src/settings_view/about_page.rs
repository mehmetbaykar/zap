use super::{
    settings_page::{
        render_body_item, MatchData, PageType, SettingsPageEvent, SettingsPageMeta,
        SettingsPageViewHandle, SettingsWidget,
    },
    LocalOnlyIconState, SettingsSection, ToggleState,
};
use crate::{
    appearance::Appearance,
    autoupdate::{self, github, AutoupdateStage, AutoupdateState},
    channel::ChannelState,
    report_if_error,
    settings::AutoupdateSettings,
    workspace::WorkspaceAction,
};
use settings::Setting as _;
use warp_core::{execution_mode::AppExecutionMode, settings::ToggleableSetting as _};
use warpui::ui_components::switch::SwitchStateHandle;
use warpui::{
    assets::asset_cache::AssetSource,
    elements::{
        Align, CacheOption, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, Image,
        MainAxisAlignment, MouseStateHandle, ParentElement, Wrap,
    },
    ui_components::components::UiComponent,
    AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
};

#[derive(Debug, Clone)]
pub enum AboutPageAction {
    ToggleAutomaticUpdates,
    /// User clicked the "Check for Updates" button: actively triggers one check (equivalent to RequestType::ManualCheck).
    CheckForUpdate,
    /// User clicked the "Download from GitHub" link: opens the release page in the system default browser.
    /// Only used in the exceptional fallback path (e.g. download failed / no available asset).
    OpenReleasePage(String),
    /// User clicked the "Install Now" link: dispatched to the workspace, triggering the install + restart
    /// flow that is fully equivalent to the menu `ApplyUpdate`. See `autoupdate::apply_update` for platform-specific behavior.
    InstallUpdate,
    /// User clicked the "Export Logs" link: pops up the native save-file dialog. After the user picks a save
    /// location, the main log, MCP log, auto-updater log, and diagnostic summary are packaged into a zip
    /// and written directly to the user-specified path, reporting success / failure via a workspace toast on completion.
    /// Implemented by `WorkspaceAction::ExportLogsToPath`.
    #[cfg(not(target_family = "wasm"))]
    ExportLogs,
}

pub struct AboutPageView {
    page: PageType<Self>,
}

impl AboutPageView {
    pub fn new(ctx: &mut ViewContext<AboutPageView>) -> Self {
        // Subscribe to AutoupdateState; refresh the UI when the stage changes (checking / new version found / failed, etc.).
        let autoupdate_handle = AutoupdateState::handle(ctx);
        ctx.observe(&autoupdate_handle, |_, _, ctx| {
            ctx.notify();
        });

        AboutPageView {
            page: PageType::new_monolith(AboutPageWidget::default(), None, false),
        }
    }
}

impl Entity for AboutPageView {
    type Event = SettingsPageEvent;
}

impl TypedActionView for AboutPageView {
    type Action = AboutPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            AboutPageAction::ToggleAutomaticUpdates => {
                AutoupdateSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .automatic_updates_enabled
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AboutPageAction::CheckForUpdate => {
                AutoupdateState::handle(ctx).update(ctx, |state, ctx| {
                    state.manually_check_for_update(ctx);
                });
                ctx.notify();
            }
            AboutPageAction::OpenReleasePage(url) => {
                ctx.open_url(url);
            }
            AboutPageAction::InstallUpdate => {
                // Reuse WorkspaceAction::ApplyUpdate: it calls autoupdate::apply_update +
                // initiate_relaunch_for_update, and the platform layer decides the concrete install action in relaunch()
                // (mac OSS: open dmg / Win OSS: non-silent install wizard / Linux: restart the new binary).
                ctx.dispatch_typed_action(&WorkspaceAction::ApplyUpdate);
            }
            #[cfg(not(target_family = "wasm"))]
            AboutPageAction::ExportLogs => {
                // Trigger the workspace layer to pop up the save-file dialog; after the user picks a save path,
                // it completes packaging and reports back via a toast.
                ctx.dispatch_typed_action(&WorkspaceAction::ExportLogsToPath);
            }
        }
    }
}

impl View for AboutPageView {
    fn ui_name() -> &'static str {
        "AboutPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

#[derive(Default)]
struct AboutPageWidget {
    copy_version_button_mouse_state: MouseStateHandle,
    automatic_updates_switch_state: SwitchStateHandle,
    update_action_link_mouse_state: MouseStateHandle,
    /// Hover / pressed state of the "Export Logs" link.
    #[cfg(not(target_family = "wasm"))]
    export_logs_link_mouse_state: MouseStateHandle,
}

impl SettingsWidget for AboutPageWidget {
    type View = AboutPageView;

    fn search_terms(&self) -> &str {
        "about warp version automatic updates auto update check for update new version"
    }

    fn render(
        &self,
        _view: &AboutPageView,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ui_builder = appearance.ui_builder();

        // Always use the icon-only logo; the brand name is rendered as standalone "Zap" text, no longer relying on an svg containing the "warp" wordmark
        let image_path = "bundled/svg/warp-logo-light.svg";

        // GIT_RELEASE_TAG injected → display the tag; otherwise fall back to Dev development mode
        let version = ChannelState::app_version().unwrap_or("Dev");

        let version_text = ui_builder
            .span(version.to_string())
            .with_soft_wrap()
            .build()
            .with_margin_top(16.)
            .finish();

        let copy_version_icon = appearance
            .ui_builder()
            .copy_button(16., self.copy_version_button_mouse_state.clone())
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::CopyVersion(version));
            })
            .finish();

        let version_row = Wrap::row()
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_children([
                version_text,
                Container::new(copy_version_icon)
                    .with_margin_top(16.)
                    .with_padding_left(6.)
                    .finish(),
            ]);

        let mut content = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                ConstrainedBox::new(
                    Image::new(
                        AssetSource::Bundled { path: image_path },
                        CacheOption::BySize,
                    )
                    .finish(),
                )
                .with_max_height(100.)
                .with_max_width(350.)
                .finish(),
            )
            .with_child(ui_builder.span("Zap").build().with_margin_top(12.).finish())
            .with_child(version_row.finish());

        // Update status area: shows whether a new version is currently available, and provides a "Check for Updates" or "Download from GitHub" link.
        // Only rendered in execution modes that can enter the autoupdate flow (shares the same condition as the "Automatic Updates" toggle below).
        if AppExecutionMode::as_ref(app).can_autoupdate() {
            content.add_child(
                Container::new(self.render_update_status(appearance, app))
                    .with_margin_top(16.)
                    .finish(),
            );
        }

        content.add_child(
            ui_builder
                .span(crate::t!("settings-about-copyright"))
                .build()
                .with_margin_top(16.)
                .finish(),
        );

        // "Export Logs" link: platform-native export of a zip to share with troubleshooters. WASM platforms have no
        // filesystem logs, so skip it.
        #[cfg(not(target_family = "wasm"))]
        {
            let export_link = ui_builder
                .link(
                    crate::t!("settings-about-export-logs"),
                    None,
                    Some(Box::new(|ctx| {
                        ctx.dispatch_typed_action(AboutPageAction::ExportLogs);
                    })),
                    self.export_logs_link_mouse_state.clone(),
                )
                .soft_wrap(false)
                .build()
                .finish();

            // Use a vertical Flex column to present both the link and the descriptive text (explaining why to export and what is included).
            let export_section = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(export_link)
                .with_child(
                    ui_builder
                        .span(crate::t!("settings-about-export-logs-description"))
                        .with_soft_wrap()
                        .build()
                        .with_margin_top(4.)
                        .finish(),
                )
                .finish();

            content.add_child(Container::new(export_section).with_margin_top(16.).finish());
        }

        if AppExecutionMode::as_ref(app).can_autoupdate() {
            content.add_child(
                Container::new(
                    ConstrainedBox::new(render_body_item::<AboutPageAction>(
                        crate::t!("settings-about-automatic-updates-label"),
                        None,
                        LocalOnlyIconState::Hidden,
                        ToggleState::Enabled,
                        appearance,
                        appearance
                            .ui_builder()
                            .switch(self.automatic_updates_switch_state.clone())
                            .check(
                                *AutoupdateSettings::as_ref(app)
                                    .automatic_updates_enabled
                                    .value(),
                            )
                            .build()
                            .on_click(move |ctx, _, _| {
                                ctx.dispatch_typed_action(AboutPageAction::ToggleAutomaticUpdates);
                            })
                            .finish(),
                        Some(crate::t!("settings-about-automatic-updates-description")),
                    ))
                    .with_max_width(520.)
                    .finish(),
                )
                .with_margin_top(24.)
                .finish(),
            );
        }

        Align::new(content.finish()).finish()
    }
}

impl AboutPageWidget {
    /// Render the "Update status" row: status text + action link (check for update / progress display / install now / GitHub fallback).
    fn render_update_status(&self, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
        let ui_builder = appearance.ui_builder();

        // The current stage determines the copy and the action:
        // - NoUpdateAvailable / unknown error: already up to date + "Check for Updates"
        // - CheckingForUpdate: checking... (no action)
        // - DownloadingUpdate: downloading X% (X MB / Y MB) (no action)
        // - UpdateReady / UpdatedPendingRestart: ready to install + "Install Now" button
        // - UnableTo*: automatic install failed + "Download from GitHub" fallback link
        let stage = autoupdate::get_update_state(app);
        let progress = autoupdate::AutoupdateState::as_ref(app)
            .download_progress()
            .cloned();

        let (status_text, action) = match &stage {
            AutoupdateStage::CheckingForUpdate => (
                crate::t!("settings-about-update-checking"),
                UpdateAction::None,
            ),
            AutoupdateStage::DownloadingUpdate => {
                // Shared across all three platforms: take the downloaded bytes from AutoupdateState.download_progress
                // and format them as "X.X MB / Y.Y MB (P%)"; when the total size is unknown, only show the downloaded bytes.
                let new_version = stage
                    .available_new_version()
                    .map(|v| v.version.as_str())
                    .unwrap_or("");
                let text = match &progress {
                    Some(p) => {
                        // i18n_embed_fl::fl! requires arguments to be references with a lifetime, so
                        // bind the progress string to a let first; don't pass a temporary expression.
                        let progress_str = format_download_progress(p);
                        crate::t!(
                            "settings-about-update-downloading",
                            version = new_version,
                            progress = progress_str.as_str()
                        )
                    }
                    None => crate::t!(
                        "settings-about-update-downloading-init",
                        version = new_version
                    ),
                };
                (text, UpdateAction::None)
            }
            AutoupdateStage::NoUpdateAvailable => (
                crate::t!("settings-about-update-up-to-date"),
                UpdateAction::Check,
            ),
            AutoupdateStage::UpdateReady { new_version, .. }
            | AutoupdateStage::UpdatedPendingRestart { new_version } => {
                let text = crate::t!(
                    "settings-about-update-ready",
                    version = new_version.version.as_str()
                );
                (text, UpdateAction::Install)
            }
            stage if stage.available_new_version().is_some() => {
                // UnableToUpdateToNewVersion / UnableToLaunchNewVersion / Updating (leftover):
                // automatic install errored out or was interrupted → give the user a manual download fallback.
                let new_version = stage.available_new_version().unwrap();
                let text = crate::t!(
                    "settings-about-update-available",
                    version = new_version.version.as_str()
                );
                let url = github::cached_release()
                    .map(|r| r.html_url)
                    .unwrap_or_else(|| {
                        "https://github.com/zerx-lab/warp/releases/latest".to_owned()
                    });
                (text, UpdateAction::OpenReleasePage(url))
            }
            // Fallback (theoretically unreachable): treat any remaining stage as "already up to date".
            _ => (
                crate::t!("settings-about-update-up-to-date"),
                UpdateAction::Check,
            ),
        };

        let mut row = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(ui_builder.span(status_text).build().finish());

        match action {
            UpdateAction::None => {}
            UpdateAction::Check => {
                row.add_child(
                    Container::new(
                        ui_builder
                            .link(
                                crate::t!("settings-about-update-check-now"),
                                None,
                                Some(Box::new(|ctx| {
                                    ctx.dispatch_typed_action(AboutPageAction::CheckForUpdate);
                                })),
                                self.update_action_link_mouse_state.clone(),
                            )
                            .soft_wrap(false)
                            .build()
                            .finish(),
                    )
                    .with_padding_left(8.)
                    .finish(),
                );
            }
            UpdateAction::OpenReleasePage(url) => {
                let url_clone = url.clone();
                row.add_child(
                    Container::new(
                        ui_builder
                            .link(
                                crate::t!("settings-about-update-open-release"),
                                None,
                                Some(Box::new(move |ctx| {
                                    ctx.dispatch_typed_action(AboutPageAction::OpenReleasePage(
                                        url_clone.clone(),
                                    ));
                                })),
                                self.update_action_link_mouse_state.clone(),
                            )
                            .soft_wrap(false)
                            .build()
                            .finish(),
                    )
                    .with_padding_left(8.)
                    .finish(),
                );
            }
            UpdateAction::Install => {
                row.add_child(
                    Container::new(
                        ui_builder
                            .link(
                                crate::t!("settings-about-update-install-now"),
                                None,
                                Some(Box::new(|ctx| {
                                    ctx.dispatch_typed_action(AboutPageAction::InstallUpdate);
                                })),
                                self.update_action_link_mouse_state.clone(),
                            )
                            .soft_wrap(false)
                            .build()
                            .finish(),
                    )
                    .with_padding_left(8.)
                    .finish(),
                );
            }
        }

        // Install hint: only shown in the UpdateReady/UpdatedPendingRestart states (the Install action),
        // so the user knows in advance what they'll see next before clicking (open dmg / launch install wizard / restart AppImage).
        if matches!(
            autoupdate::get_update_state(app),
            AutoupdateStage::UpdateReady { .. } | AutoupdateStage::UpdatedPendingRestart { .. }
        ) {
            // t! is a macro and must be passed a literal, not a variable. Pick the specific key per cfg branch.
            #[cfg(target_os = "macos")]
            let hint = crate::t!("settings-about-update-install-hint-macos");
            #[cfg(windows)]
            let hint = crate::t!("settings-about-update-install-hint-windows");
            #[cfg(all(not(target_os = "macos"), not(windows)))]
            let hint = crate::t!("settings-about-update-install-hint-linux");

            return Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(row.finish())
                .with_child(
                    ui_builder
                        .span(hint)
                        .with_soft_wrap()
                        .build()
                        .with_margin_top(4.)
                        .finish(),
                )
                .finish();
        }

        row.finish()
    }
}

/// Format a byte count as "X.X MB" / "X KB", used for download progress copy.
fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Render DownloadProgress as "1.2 MB / 3.4 MB (35%)"; when total is unknown, only show what's downloaded.
fn format_download_progress(p: &autoupdate::DownloadProgress) -> String {
    let downloaded = format_bytes(p.downloaded);
    match p.total {
        Some(total) if total > 0 => {
            let pct = ((p.downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0);
            format!("{} / {} ({:.0}%)", downloaded, format_bytes(total), pct)
        }
        _ => downloaded,
    }
}

/// The action shown in the update status area: none / check for update / open GitHub Release / install now.
enum UpdateAction {
    None,
    Check,
    OpenReleasePage(String),
    Install,
}

impl SettingsPageMeta for AboutPageView {
    fn section() -> SettingsSection {
        SettingsSection::About
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        true
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

impl From<ViewHandle<AboutPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<AboutPageView>) -> Self {
        SettingsPageViewHandle::About(view_handle)
    }
}
