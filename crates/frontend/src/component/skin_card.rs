use gpui::{prelude::*, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, Window, *};
use gpui_component::StyledExt;
use std::sync::Arc;

pub fn render_skin_card(
    skin_id: Arc<str>,
    is_active: bool,
    url: Arc<str>,
    variant: Arc<str>,
    front_image: Option<Arc<RenderImage>>,
    back_image: Option<Arc<RenderImage>>,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(format!("skin-card-{}", skin_id))
        .w(px(155.0))
        .h(px(155.0))
        .bg(gpui::rgba(0x2d2d35ff))
        .rounded_lg()
        .border_2()
        .border_color(if is_active {
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
        .when(is_active, |this| {
            this.child(
                div()
                    .absolute()
                    .top_2()
                    .right_2()
                    .bg(gpui::rgba(0x00ff00ff))
                    .text_color(gpui::black())
                    .text_xs()
                    .px_1()
                    .rounded_sm()
                    .child("Equipped"),
            )
        })
}
