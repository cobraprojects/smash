use std::cell::RefCell;
use std::collections::HashMap;

use settings::{Setting, ToggleableSetting};
use warp_errors::report_if_error;
use warpui::elements::Element;
use warpui::ui_components::components::UiComponent;
use warpui::ui_components::switch::SwitchStateHandle;
use warpui::{
    AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle, WindowId,
};

use super::SettingsSection;
use super::settings_page::{
    Category, LocalOnlyIconState, MatchData, PageType, SettingsPageEvent, SettingsPageMeta,
    SettingsPageViewHandle, SettingsWidget, ToggleState, render_body_item,
};
use crate::appearance::Appearance;
use crate::workspace::tab_settings::{
    SessionSidebarCompactPaths, SessionSidebarShowDetails, SessionSidebarShowGitBranch,
    SessionSidebarShowTabCount, SessionSidebarShowWorkingDirectory,
    ShowVerticalTabPanelInRestoredWindows, TabSettings, UseVerticalTabs,
};
use crate::workspace::{WorkspaceAction, WorkspaceRegistry};

pub struct SidebarSettingsPageView {
    page: PageType<Self>,
    window_id: WindowId,
    local_only_icon_tooltip_states: RefCell<HashMap<String, warpui::elements::MouseStateHandle>>,
}

impl SidebarSettingsPageView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let workspace_registry = WorkspaceRegistry::handle(ctx);
        ctx.subscribe_to_model(&workspace_registry, |_view, _registry, (), ctx| {
            ctx.notify();
        });

        Self {
            page: PageType::new_categorized(
                vec![
                    Category::new(
                        "Layout",
                        vec![
                            Box::new(EnableSessionSidebarWidget::default()),
                            Box::new(ToggleSidebarVisibilityWidget::default()),
                            Box::new(OpenSidebarOnRestoreWidget::default()),
                        ],
                    )
                    .with_subtitle(
                        "The session sidebar and horizontal tab bar are independent surfaces.",
                    ),
                    Category::new(
                        "Session details",
                        vec![
                            Box::new(ShowDetailsWidget::default()),
                            Box::new(ShowTabCountWidget::default()),
                            Box::new(ShowWorkingDirectoryWidget::default()),
                            Box::new(ShowGitBranchWidget::default()),
                            Box::new(CompactPathsWidget::default()),
                        ],
                    )
                    .with_subtitle(
                        "Choose the metadata shown beneath each session in the sidebar.",
                    ),
                ],
                Some("Sidebar"),
            ),
            window_id: ctx.window_id(),
            local_only_icon_tooltip_states: RefCell::default(),
        }
    }
}

impl Entity for SidebarSettingsPageView {
    type Event = SettingsPageEvent;
}

impl View for SidebarSettingsPageView {
    fn ui_name() -> &'static str {
        "SidebarSettingsPageView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SidebarPageAction {
    ToggleEnabled,
    ToggleOpenOnRestore,
    ToggleDetails,
    ToggleTabCount,
    ToggleWorkingDirectory,
    ToggleGitBranch,
    ToggleCompactPaths,
}

impl TypedActionView for SidebarSettingsPageView {
    type Action = SidebarPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        TabSettings::handle(ctx).update(ctx, |settings, ctx| {
            let result = match action {
                SidebarPageAction::ToggleEnabled => {
                    settings.use_vertical_tabs.toggle_and_save_value(ctx)
                }
                SidebarPageAction::ToggleOpenOnRestore => settings
                    .show_vertical_tab_panel_in_restored_windows
                    .toggle_and_save_value(ctx),
                SidebarPageAction::ToggleDetails => settings
                    .session_sidebar_show_details
                    .toggle_and_save_value(ctx),
                SidebarPageAction::ToggleTabCount => settings
                    .session_sidebar_show_tab_count
                    .toggle_and_save_value(ctx),
                SidebarPageAction::ToggleWorkingDirectory => settings
                    .session_sidebar_show_working_directory
                    .toggle_and_save_value(ctx),
                SidebarPageAction::ToggleGitBranch => settings
                    .session_sidebar_show_git_branch
                    .toggle_and_save_value(ctx),
                SidebarPageAction::ToggleCompactPaths => settings
                    .session_sidebar_compact_paths
                    .toggle_and_save_value(ctx),
            };
            report_if_error!(result);
        });
        ctx.notify();
    }
}

impl SettingsPageMeta for SidebarSettingsPageView {
    fn section() -> SettingsSection {
        SettingsSection::Sidebar
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

impl From<ViewHandle<SidebarSettingsPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<SidebarSettingsPageView>) -> Self {
        SettingsPageViewHandle::Sidebar(view_handle)
    }
}

fn local_only<S: Setting>(view: &SidebarSettingsPageView, app: &AppContext) -> LocalOnlyIconState {
    LocalOnlyIconState::for_setting(
        S::storage_key(),
        S::sync_to_cloud(),
        &mut view.local_only_icon_tooltip_states.borrow_mut(),
        app,
    )
}

fn render_switch_row(
    title: &str,
    description: &str,
    checked: bool,
    switch_state: SwitchStateHandle,
    action: SidebarPageAction,
    local_only_state: LocalOnlyIconState,
    appearance: &Appearance,
) -> Box<dyn Element> {
    render_body_item::<SidebarPageAction>(
        title.to_string(),
        None,
        local_only_state,
        ToggleState::Enabled,
        appearance,
        appearance
            .ui_builder()
            .switch(switch_state)
            .check(checked)
            .build()
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
            .finish(),
        Some(description.to_string()),
    )
}

macro_rules! sidebar_toggle_widget {
    ($name:ident, $terms:literal, $title:literal, $description:literal, $field:ident, $setting:ty, $action:expr) => {
        #[derive(Default)]
        struct $name {
            switch_state: SwitchStateHandle,
        }

        impl SettingsWidget for $name {
            type View = SidebarSettingsPageView;

            fn search_terms(&self) -> &str {
                $terms
            }

            fn render(
                &self,
                view: &Self::View,
                appearance: &Appearance,
                app: &AppContext,
            ) -> Box<dyn Element> {
                render_switch_row(
                    $title,
                    $description,
                    *TabSettings::as_ref(app).$field,
                    self.switch_state.clone(),
                    $action,
                    local_only::<$setting>(view, app),
                    appearance,
                )
            }
        }
    };
}

sidebar_toggle_widget!(
    EnableSessionSidebarWidget,
    "session sidebar enable layout horizontal tabs",
    "Use sessions and top tabs",
    "Organize tabs into sessions and show the active session's tabs across the top. Sidebar visibility remains independent.",
    use_vertical_tabs,
    UseVerticalTabs,
    SidebarPageAction::ToggleEnabled
);

#[derive(Default)]
struct ToggleSidebarVisibilityWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for ToggleSidebarVisibilityWidget {
    type View = SidebarSettingsPageView;

    fn search_terms(&self) -> &str {
        "sidebar show hide toggle visibility current window"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let checked = WorkspaceRegistry::as_ref(app)
            .is_sidebar_visible(view.window_id)
            .unwrap_or(false);
        let switch = appearance
            .ui_builder()
            .switch(self.switch_state.clone())
            .check(checked)
            .build()
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::ToggleVerticalTabsPanel);
            })
            .finish();

        render_body_item::<SidebarPageAction>(
            "Show or hide sidebar".to_string(),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            switch,
            Some(
                "Toggle the sidebar in this window without hiding or changing the top tabs."
                    .to_string(),
            ),
        )
    }
}
sidebar_toggle_widget!(
    OpenSidebarOnRestoreWidget,
    "sidebar open restore launch window visibility",
    "Open sidebar in restored windows",
    "Open the session sidebar when a saved window is restored. You can still show or hide it at any time.",
    show_vertical_tab_panel_in_restored_windows,
    ShowVerticalTabPanelInRestoredWindows,
    SidebarPageAction::ToggleOpenOnRestore
);
sidebar_toggle_widget!(
    ShowDetailsWidget,
    "sidebar session details metadata hide all",
    "Show session details",
    "Display enabled metadata beneath session names.",
    session_sidebar_show_details,
    SessionSidebarShowDetails,
    SidebarPageAction::ToggleDetails
);
sidebar_toggle_widget!(
    ShowTabCountWidget,
    "sidebar session tab count",
    "Show tab count",
    "Display how many tabs belong to each session.",
    session_sidebar_show_tab_count,
    SessionSidebarShowTabCount,
    SidebarPageAction::ToggleTabCount
);
sidebar_toggle_widget!(
    ShowWorkingDirectoryWidget,
    "sidebar session cwd path working directory project",
    "Show working directory",
    "Display the working directory of the session's last active tab.",
    session_sidebar_show_working_directory,
    SessionSidebarShowWorkingDirectory,
    SidebarPageAction::ToggleWorkingDirectory
);
sidebar_toggle_widget!(
    ShowGitBranchWidget,
    "sidebar session git branch repository",
    "Show git branch",
    "Display the git branch of the session's last active tab.",
    session_sidebar_show_git_branch,
    SessionSidebarShowGitBranch,
    SidebarPageAction::ToggleGitBranch
);
sidebar_toggle_widget!(
    CompactPathsWidget,
    "sidebar session path compact last segment truncate",
    "Compact working-directory paths",
    "Show only the trailing directory name instead of the full abbreviated path.",
    session_sidebar_compact_paths,
    SessionSidebarCompactPaths,
    SidebarPageAction::ToggleCompactPaths
);
