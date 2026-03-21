use gpui::{prelude::*, InteractiveElement, IntoElement, ParentElement, Styled, Window, *};
use gpui_component::{Sizable, button::{Button, ButtonVariants}, h_flex};
use std::sync::Arc;

use crate::icon::PandoraIcon;

pub fn render_skin_card(
    skin_id: Arc<str>,
    is_selected: bool,
    is_equipped: bool,
    url: Arc<str>,
    variant: Arc<str>,
    front_image: Option<Arc<RenderImage>>,
    back_image: Option<Arc<RenderImage>>,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    on_reveal: Option<impl Fn(&mut Window, &mut App) + 'static>,
    on_delete: Option<impl Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
) -> impl IntoElement {
    let action_buttons = on_reveal.map(|on_reveal| {
        let reveal_button = Button::new(format!("reveal-{}", skin_id))
            .icon(PandoraIcon::FolderOpen)
            .small()
            .compact()
            .on_mouse_down(MouseButton::Left, move |_, _, cx: &mut App| {
                cx.stop_propagation();
            })
            .on_click(move |_, window, cx| {
                on_reveal(window, cx);
            });

        let row = h_flex()
            .absolute()
            .top_2()
            .right_2()
            .gap_1()
            .invisible()
            .group_hover("skin-card", |style| style.visible())
            .child(reveal_button);
        if let Some(on_delete) = on_delete {
            row.child(
                Button::new(format!("delete-{}", skin_id))
                    .icon(PandoraIcon::Trash2)
                    .danger()
                    .small()
                    .compact()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx: &mut App| {
                        cx.stop_propagation();
                    })
                    .on_click(move |click, window, cx| {
                        on_delete(click, window, cx);
                    }),
            )
        } else {
            row
        }
    });

    div()
        .id(format!("skin-card-{}", skin_id))
        .w(px(155.0))
        .h(px(155.0))
        .bg(gpui::rgba(0x2d2d35ff))
        .rounded_lg()
        .border_2()
        .border_color(if is_selected {
            gpui::rgba(0x00ff00ff)
        } else {
            gpui::rgba(0x00000000)
        })
        .relative()
        .cursor_pointer()
        .group("skin-card")
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(window, cx);
        })
        .child(
            div()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    // Front image
                    div()
                        .size_full()
                        .group_hover("skin-card", |style| style.invisible())
                        .when_some(front_image.clone(), |this, img| {
                            this.child(
                                canvas(
                                    move |_, _, _| (),
                                    move |bounds, _, window, _| {
                                        let _ =
                                            window.paint_image(bounds, gpui::Corners::default(), img.clone(), 0, false);
                                    },
                                )
                                .size_full(),
                            )
                        })
                        .when(front_image.as_ref().is_none(), |this| this.child("Loading...")),
                )
                .child(
                    // Back image (on hover)
                    div()
                        .size_full()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover("skin-card", |style| style.visible())
                        .when_some(back_image, |this, img| {
                            this.child(
                                canvas(
                                    move |_, _, _| (),
                                    move |bounds, _, window, _| {
                                        let _ =
                                            window.paint_image(bounds, gpui::Corners::default(), img.clone(), 0, false);
                                    },
                                )
                                .size_full(),
                            )
                        }),
                ),
        )
        .when(is_equipped || is_selected, |this| {
            this.child(
                div()
                    .absolute()
                    .top_2()
                    .left_2()
                    .bg(gpui::rgba(0x00ff00ff))
                    .text_color(gpui::black())
                    .text_xs()
                    .px_1()
                    .rounded_sm()
                    .child(if is_equipped { "Equipped" } else { "Selected" }),
            )
        })
        .when_some(action_buttons, |this, btns| this.child(btns))
}
