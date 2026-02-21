use std::sync::{atomic::{AtomicBool, AtomicU8, Ordering}, Arc};

use bridge::{handle::BackendHandle, instance::InstanceID, message::EmbeddedOrRaw, modal_action::ModalAction};
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariants}, h_flex, input::{Input, InputEvent, InputState}, scroll::ScrollableElement, v_flex, Disableable, Icon, IconName, Sizable, StyleSized, WindowExt
};
use parking_lot::RwLock;

const MINECRAFT_ICON_PATHS: &[&str] = &[
    "images/grass-block-icon.png",
    "images/diamond-sword-icon.png",
    "images/diamond-pickaxe-icon.png",
    "images/creeper-face-icon.png",
    "images/ender-pearl-icon.png",
    "images/nether-star-icon.png",
    "images/tnt-icon.png",
    "images/obsidian-block-icon.png",
    "images/enchanted-golden-apple-icon.png",
];

pub fn open_select_icon(
    selected: Box<dyn FnOnce(Option<EmbeddedOrRaw>, &mut App)>,
    window: &mut Window,
    cx: &mut App,
) {
    let select_file_task = Arc::new(RwLock::new(Task::ready(())));
    let selected = Arc::new(RwLock::new(Some(selected)));
    window.open_dialog(cx, move |dialog, _, _| {
        let minecraft_icons = MINECRAFT_ICON_PATHS.iter().enumerate().filter_map(|(index, icon_path)| {
            let data = crate::Assets::get(*icon_path)?.data;
            Some((index, *icon_path, data))
        }).map(|(index, icon_path, icon_data)| {
            let icon_data: Arc<[u8]> = Arc::from(icon_data);
            Button::new(("minecraft", index)).success().with_size(px(64.0))
                .child(gpui::img(ImageSource::Resource(Resource::Embedded(icon_path.into()))).w_16().h_16())
                .on_click({
                    let selected = selected.clone();
                    let icon_data = icon_data.clone();
                    move |_, window, cx| {
                        cx.stop_propagation();
                        if let Some(selected) = selected.write().take() {
                            (selected)(Some(EmbeddedOrRaw::Raw(icon_data.clone())), cx);
                        }
                        window.close_dialog(cx);
                    }
                })
        });

        let minecraft_grid = div()
            .grid()
            .grid_cols(6)
            .w_full()
            .gap_2()
            .children(minecraft_icons);

        let icons = ICONS.iter().enumerate().map(|(index, icon)| {
            let icon = *icon;
            Button::new(index).success().icon(Icon::default().path(icon)).with_size(px(64.0)).on_click({
                let selected = selected.clone();
                move |_, window, cx| {
                    if let Some(selected) = selected.write().take() {
                        (selected)(Some(EmbeddedOrRaw::Embedded(icon.into())), cx);
                    }
                    window.close_dialog(cx);
                }
            })
        });

        let grid = div()
            .grid()
            .grid_cols(6)
            .w_full()
            .max_h_128()
            .gap_2()
            .children(icons);

        let content = v_flex()
            .size_full()
            .gap_2()
            .child(h_flex()
                .gap_2()
                .child(Button::new("reset").danger().label("Reset").icon(Icon::default().path("icons/refresh-ccw.svg")).on_click({
                    let selected = selected.clone();
                    move |_, window, cx| {
                        if let Some(selected) = selected.write().take() {
                            (selected)(None, cx);
                        }
                        window.close_dialog(cx);
                    }
                }))
                .child(Button::new("custom").success().label("Custom").icon(IconName::File).on_click({
                    let selected = selected.clone();
                    let select_file_task = select_file_task.clone();
                    move |_, window, cx| {
                        let receiver = cx.prompt_for_paths(PathPromptOptions {
                            files: true,
                            directories: false,
                            multiple: false,
                            prompt: Some(SharedString::new_static("Select PNG Icon"))
                        });

                        let selected = selected.clone();
                        *select_file_task.write() = window.spawn(cx, async move |cx| {
                            let Ok(Ok(Some(result))) = receiver.await else {
                                return;
                            };
                            let Some(path) = result.first() else {
                                return;
                            };
                            let Ok(bytes) = std::fs::read(path) else {
                                return;
                            };
                            _ = cx.update(move |window, cx| {
                                if let Some(selected) = selected.write().take() {
                                    (selected)(Some(EmbeddedOrRaw::Raw(bytes.into())), cx);
                                }
                                window.close_dialog(cx);
                            });
                        });
                    }
                })))
            .child(minecraft_grid)
            .child(grid);

        dialog
            .title("Select Icon")
            .child(content)
    });

}

static ICONS: &[&'static str] = &[
    "icons/box.svg",
    "icons/swords.svg",
    "icons/camera.svg",
    "icons/brush.svg",
    "icons/house.svg",
    "icons/anvil.svg",
    "icons/archive.svg",
    "icons/asterisk.svg",
    "icons/award.svg",
    "icons/book.svg",
    "icons/bot.svg",
    "icons/briefcase.svg",
    "icons/bug.svg",
    "icons/building-2.svg",
    "icons/carrot.svg",
    "icons/cat.svg",
    "icons/compass.svg",
    "icons/cpu.svg",
    "icons/dollar-sign.svg",
    "icons/eye.svg",
    "icons/feather.svg",
    "icons/heart.svg",
    "icons/moon.svg",
    "icons/palette.svg",
    "icons/scroll.svg",
    "icons/square-terminal.svg",
    "icons/tree-pine.svg",
    "icons/wand-sparkles.svg",
    "icons/zap.svg",
];
