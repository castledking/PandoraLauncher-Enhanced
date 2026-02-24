use gpui::{prelude::*, InteractiveElement, IntoElement, ParentElement, Styled, Window, *};
use std::sync::Arc;

pub fn render_cape_card(
    cape_id: Arc<str>,
    is_active: bool,
    _front_image: Option<Arc<RenderImage>>,
    back_image: Option<Arc<RenderImage>>,
    label: &'static str,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(format!("cape-card-{}", cape_id))
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
        .group("cape-card")
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(window, cx);
        })
        .child({
            let has_back = back_image.is_some();
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
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
                })
                .when(!has_back, |this| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size_full()
                            .text_sm()
                            .text_color(gpui::rgba(0x888888ff))
                            .child(label),
                    )
                })
        })
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
