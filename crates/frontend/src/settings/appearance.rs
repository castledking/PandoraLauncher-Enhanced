use std::rc::Rc;

use std::path::Path;

use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, Sizable, ThemeRegistry, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    notification::Notification,
    select::{Select, SelectEvent},
    slider::{Slider, SliderEvent, SliderState},
    v_flex,
};
use strum::IntoEnumIterator;

use crate::{
    component::{
        background_effect::{self, BackgroundEffect, EffectPalette, EffectQuality},
        named_dropdown::{DropdownName, NamedDropdownItem, SearchableNamedDropdown},
    },
    entity::DataEntities,
    icon::PandoraIcon,
    interface_config::InterfaceConfig,
    settings::{SettingGroup, SettingItem, SettingItemWidget, SettingPage, SettingsRoot},
};

pub(super) fn create_page(data: &DataEntities, window: &mut Window, cx: &mut App) -> SettingPage {
    SettingPage {
        title: t::settings::appearance,
        groups: vec![
            SettingGroup {
                title: Some(t::settings::appearance::theme),
                items: vec![
                    SettingItem {
                        title: t::settings::appearance::theme::base_theme,
                        description: t::settings::appearance::theme::base_theme_desc,
                        widget: create_theme_dropdown(window, cx),
                        ..Default::default()
                    },
                    SettingItem {
                        title: t::settings::appearance::theme::open_theme_folder,
                        description: t::settings::appearance::theme::open_theme_folder_desc,
                        widget: SettingItemWidget::Any(Rc::new({
                            let theme_folder = data.theme_folder.clone();
                            move |_, _| {
                                Button::new("open-theme-folder")
                                    .info()
                                    .icon(PandoraIcon::FolderOpen)
                                    .label(t::settings::appearance::theme::open_theme_folder())
                                    .on_click({
                                        let theme_folder = theme_folder.clone();
                                        move |_, window, cx| {
                                            crate::open_folder(&theme_folder, window, cx);
                                        }
                                    })
                                    .into_any_element()
                            }
                        })),
                        ..Default::default()
                    },
                    SettingItem {
                        title: t::settings::appearance::theme::font,
                        description: t::settings::appearance::theme::font_desc,
                        widget: create_font_dropdown(window, cx),
                        ..Default::default()
                    },
                    SettingItem {
                        title: t::settings::appearance::theme::font_size,
                        description: t::settings::appearance::theme::font_size_desc,
                        widget: SettingItemWidget::Integer {
                            get: |cx| cx.theme().font_size.as_f32().round() as i32,
                            set: |val, cx| {
                                InterfaceConfig::get_mut(cx).set_font_size(val);
                                InterfaceConfig::apply_theme(cx, true);
                            },
                            min: Some(8),
                            max: Some(32),
                        },
                        ..Default::default()
                    },
                ]
                .into(),
                searched_items: None,
            },
            SettingGroup {
                title: Some(t::settings::appearance::window),
                items: vec![
                    SettingItem {
                        title: t::settings::appearance::window::use_os_titlebar,
                        description: t::settings::appearance::window::use_os_titlebar_desc,
                        widget: SettingItemWidget::Switch(
                            |cfg| cfg.use_os_titlebar,
                            |cfg, val| cfg.use_os_titlebar = val,
                        ),
                        ..Default::default()
                    },
                    SettingItem {
                        title: t::settings::appearance::window::hide_main_window,
                        description: t::settings::appearance::window::hide_main_window_desc,
                        widget: SettingItemWidget::Switch(
                            |cfg| cfg.hide_main_window_on_launch,
                            |cfg, val| cfg.hide_main_window_on_launch = val,
                        ),
                        ..Default::default()
                    },
                    SettingItem {
                        title: t::settings::appearance::window::quit_when_main_window_closed,
                        description: t::settings::appearance::window::quit_when_main_window_closed_desc,
                        widget: SettingItemWidget::Switch(
                            |cfg| cfg.quit_on_main_closed,
                            |cfg, val| cfg.quit_on_main_closed = val,
                        ),
                        ..Default::default()
                    },
                ]
                .into(),
                searched_items: None,
            },
            SettingGroup {
                title: Some(t::settings::appearance::background),
                items: vec![
                    SettingItem {
                        title: t::settings::appearance::background::animated,
                        description: t::settings::appearance::background::animated_desc,
                        widget: SettingItemWidget::Switch(
                            |cfg| cfg.background_animated,
                            |cfg, val| cfg.background_animated = val,
                        ),
                        ..Default::default()
                    },
                    SettingItem {
                        title: t::settings::appearance::background::skin_animation,
                        description: t::settings::appearance::background::skin_animation_desc,
                        widget: SettingItemWidget::Switch(
                            |cfg| cfg.skin_animation,
                            |cfg, val| cfg.skin_animation = val,
                        ),
                        ..Default::default()
                    },
                    SettingItem {
                        title: t::settings::appearance::background::snow,
                        description: t::settings::appearance::background::snow_desc,
                        widget: SettingItemWidget::Switch(|cfg| cfg.snow_effect, |cfg, val| cfg.snow_effect = val),
                        ..Default::default()
                    },
                    SettingItem {
                        title: t::settings::appearance::background::quality,
                        description: t::settings::appearance::background::quality_desc,
                        widget: create_quality_slider(cx),
                        ..Default::default()
                    },
                    SettingItem {
                        title: t::settings::appearance::background::effect,
                        description: t::settings::appearance::background::effect_desc,
                        widget: SettingItemWidget::FullWidth(Rc::new(|_, cx| render_effect_picker(cx))),
                        ..Default::default()
                    },
                    SettingItem {
                        title: t::settings::appearance::background::custom,
                        description: t::settings::appearance::background::custom_desc,
                        widget: SettingItemWidget::Any(Rc::new(|_, cx| render_custom_background(cx))),
                        ..Default::default()
                    },
                ]
                .into(),
                searched_items: None,
            },
            SettingGroup {
                title: Some(t::settings::appearance::sidebar),
                items: vec![SettingItem {
                    title: t::settings::appearance::sidebar::show_sidebar_icons,
                    description: t::settings::appearance::sidebar::show_sidebar_icons_desc,
                    widget: SettingItemWidget::Switch(
                        |cfg| cfg.show_sidebar_icons,
                        |cfg, val| cfg.show_sidebar_icons = val,
                    ),
                    ..Default::default()
                }]
                .into(),
                searched_items: None,
            },
        ]
        .into(),
        searched_groups: None,
    }
}

fn create_theme_dropdown(window: &mut Window, cx: &mut App) -> SettingItemWidget {
    let themes = ThemeRegistry::global(cx)
        .sorted_themes()
        .iter()
        .map(|theme| NamedDropdownItem {
            name: DropdownName::Literal(theme.name.clone()),
            item: theme.name.clone(),
        })
        .collect();

    let initial = cx.theme().theme_name().clone();
    let dropdown = SearchableNamedDropdown::create_and_select(themes, initial, window, cx);

    cx.subscribe(&dropdown, |_, event: &SelectEvent<_>, cx| {
        let SelectEvent::Confirm(Some(theme_name)) = event else {
            return;
        };
        InterfaceConfig::get_mut(cx).set_active_theme(theme_name.clone());
        InterfaceConfig::apply_theme(cx, true);
    })
    .detach();

    SettingItemWidget::Any(Rc::new(move |_, _| Select::new(&dropdown).menu_width(px(200.0)).into_any_element()))
}

fn create_font_dropdown(window: &mut Window, cx: &mut App) -> SettingItemWidget {
    let fonts = cx
        .text_system()
        .all_font_names()
        .iter()
        .filter(|name| !name.starts_with('.'))
        .map(|font_name| {
            let font_name = SharedString::from(font_name);
            NamedDropdownItem {
                name: font_name.clone().into(),
                item: font_name,
            }
        })
        .collect();

    let initial = cx.theme().font_family.clone();
    let dropdown = SearchableNamedDropdown::create_and_select(fonts, initial, window, cx);

    cx.subscribe(&dropdown, |_, event: &SelectEvent<_>, cx| {
        let SelectEvent::Confirm(Some(font_family)) = event else {
            return;
        };

        InterfaceConfig::get_mut(cx).set_font_family(font_family.clone());
        InterfaceConfig::apply_theme(cx, true);
    })
    .detach();

    SettingItemWidget::Any(Rc::new(move |_, _| Select::new(&dropdown).menu_width(px(200.0)).into_any_element()))
}

fn create_quality_slider(cx: &mut App) -> SettingItemWidget {
    let quality = InterfaceConfig::get(cx).background_quality;
    let state = cx.new(|_| SliderState::new().min(0.0).max(2.0).step(1.0).default_value(quality.index() as f32));

    cx.subscribe(&state, |_, event: &SliderEvent, cx| {
        let SliderEvent::Change(value) = event else {
            return;
        };
        let quality = EffectQuality::from_index(value.start().round().max(0.0) as usize);
        if InterfaceConfig::get(cx).background_quality != quality {
            InterfaceConfig::get_mut(cx).background_quality = quality;
            cx.refresh_windows();
        }
    })
    .detach();

    SettingItemWidget::Any(Rc::new(move |_, cx| {
        h_flex()
            .gap_2()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(t::settings::appearance::background::quality_low())
            .child(div().w(px(110.0)).child(Slider::new(&state)))
            .child(t::settings::appearance::background::quality_high())
            .into_any_element()
    }))
}

fn render_effect_picker(cx: &mut Context<SettingsRoot>) -> AnyElement {
    let selected = InterfaceConfig::get(cx).background_effect;
    let palette = EffectPalette::from_theme(cx.theme());
    let radius = cx.theme().radius_lg;
    let (border, selected_border) = (cx.theme().border, cx.theme().blue);

    h_flex()
        .w_full()
        .flex_wrap()
        .gap_3()
        .children(BackgroundEffect::iter().map(|effect| {
            let is_selected = effect == selected;
            div()
                .id(effect.name())
                .relative()
                .w(px(148.0))
                .h(px(96.0))
                .overflow_hidden()
                .rounded(radius)
                .border_2()
                .border_color(if is_selected { selected_border } else { border })
                .cursor_pointer()
                .child(
                    background_effect::effect_canvas(effect, effect.preview_time(), EffectQuality::Low, palette, true)
                        .absolute()
                        .size_full(),
                )
                .child(div().absolute().size_full().bg(gpui::black().opacity(0.25)))
                .child(
                    v_flex()
                        .absolute()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .gap_1p5()
                        .text_color(gpui::white())
                        .child(Icon::default().path(effect.icon()).size_6())
                        .child(div().px_1().text_xs().text_center().child(effect.name().to_uppercase())),
                )
                .when(is_selected, |this| {
                    this.child(
                        div()
                            .absolute()
                            .top_1()
                            .right_1()
                            .p_0p5()
                            .rounded_full()
                            .bg(selected_border)
                            .text_color(gpui::white())
                            .child(Icon::new(PandoraIcon::Check).small()),
                    )
                })
                .on_click(move |_, _, cx| {
                    InterfaceConfig::get_mut(cx).background_effect = effect;
                    cx.refresh_windows();
                })
        }))
        .into_any_element()
}

fn render_custom_background(cx: &mut Context<SettingsRoot>) -> AnyElement {
    let current = InterfaceConfig::get(cx).custom_background.clone();
    let file_name = current
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| SharedString::from(name.to_string_lossy().into_owned()));

    h_flex()
        .gap_2()
        .when_some(file_name, |this, name| {
            this.child(div().text_sm().max_w(px(140.0)).truncate().child(name)).child(
                Button::new("remove-background")
                    .label(t::settings::appearance::background::remove())
                    .on_click(|_, _, cx| {
                        InterfaceConfig::get_mut(cx).custom_background = None;
                        cx.refresh_windows();
                    }),
            )
        })
        .child(
            Button::new("select-background")
                .info()
                .icon(PandoraIcon::Image)
                .label(t::settings::appearance::background::select_file())
                .on_click(|_, window, cx| select_custom_background(window, cx)),
        )
        .into_any_element()
}

fn select_custom_background(window: &mut Window, cx: &mut App) {
    let receiver = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(t::settings::appearance::background::select_prompt().into()),
    });

    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            _ = cx.update(|window, cx| {
                if is_supported_image(&path) {
                    InterfaceConfig::get_mut(cx).custom_background = Some(path.into());
                    cx.refresh_windows();
                } else {
                    window
                        .push_notification(Notification::error(t::settings::appearance::background::unsupported()), cx);
                }
            });
        })
        .detach();
}

/// Whether the background can be drawn from this file. Videos aren't supported.
fn is_supported_image(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    ["png", "jpg", "jpeg", "gif", "webp", "bmp"]
        .iter()
        .any(|supported| extension.eq_ignore_ascii_case(supported))
}

#[cfg(test)]
mod tests {
    // Not a glob: gpui exports its own `test` macro, which would shadow the standard one
    use std::path::Path;

    use super::is_supported_image;

    #[test]
    fn images_are_accepted_whatever_their_case() {
        assert!(is_supported_image(Path::new("/tmp/sunset.PNG")));
        assert!(is_supported_image(Path::new("/tmp/loop.gif")));
        assert!(is_supported_image(Path::new("/tmp/photo.jpeg")));
    }

    #[test]
    fn videos_and_other_files_are_turned_away() {
        assert!(!is_supported_image(Path::new("/tmp/clip.mp4")));
        assert!(!is_supported_image(Path::new("/tmp/notes.txt")));
        assert!(!is_supported_image(Path::new("/tmp/no_extension")));
    }
}
