use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{
    Align, CacheOption, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, Image,
    MainAxisAlignment, MouseStateHandle, ParentElement, Text, Wrap,
};
use warpui::fonts::{Properties, Weight};
use warpui::ui_components::components::UiComponent;
use warpui::{AppContext, Entity, View, ViewContext, ViewHandle};

use super::SettingsSection;
use super::settings_page::{
    MatchData, PageType, SettingsPageEvent, SettingsPageMeta, SettingsPageViewHandle,
    SettingsWidget,
};
use crate::appearance::Appearance;
use crate::channel::ChannelState;
use crate::themes::theme::ColorScheme;
use crate::workspace::WorkspaceAction;

pub struct AboutPageView {
    page: PageType<Self>,
}

impl AboutPageView {
    pub fn new(_ctx: &mut ViewContext<AboutPageView>) -> Self {
        AboutPageView {
            page: PageType::new_monolith(AboutPageWidget::default(), None, false),
        }
    }
}

impl Entity for AboutPageView {
    type Event = SettingsPageEvent;
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
}

impl SettingsWidget for AboutPageWidget {
    type View = AboutPageView;

    fn search_terms(&self) -> &str {
        "about smash warp version"
    }

    fn render(
        &self,
        _view: &AboutPageView,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let ui_builder = appearance.ui_builder();

        let is_smash = ChannelState::channel() == warp_core::channel::Channel::Oss;
        let image_path = if is_smash && theme.inferred_color_scheme() == ColorScheme::LightOnDark {
            "bundled/svg/smash-logo-light.svg"
        } else if is_smash {
            "bundled/svg/smash-logo-dark.svg"
        } else if theme.inferred_color_scheme() == ColorScheme::LightOnDark {
            "bundled/svg/warp-logo-with-light-title.svg"
        } else {
            "bundled/svg/warp-logo-with-dark-title.svg"
        };

        let version = ChannelState::app_version().unwrap_or("v#.##.###");

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

        let mut brand = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                ConstrainedBox::new(
                    Image::new(
                        AssetSource::Bundled { path: image_path },
                        CacheOption::BySize,
                    )
                    .finish(),
                )
                .with_max_height(if is_smash { 88. } else { 100. })
                .with_max_width(if is_smash { 120. } else { 350. })
                .finish(),
            );
        if is_smash {
            brand.add_child(
                Text::new_inline("Smash", appearance.ui_font_family(), 48.)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .finish(),
            );
        }
        Align::new(
            brand
                .with_child(version_row.finish())
                .with_child(
                    ui_builder
                        .span(if is_smash {
                            "Copyright 2026 Smash contributors"
                        } else {
                            "Copyright 2026 Warp"
                        })
                        .build()
                        .with_margin_top(16.)
                        .finish(),
                )
                .finish(),
        )
        .finish()
    }
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
