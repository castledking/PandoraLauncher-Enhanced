use std::sync::Arc;

use gpui::{prelude::*, App, Context, IntoElement, Render, Window, *};
use gpui_component::{scroll::ScrollableElement, v_flex};

use crate::{component::{page_path::PagePath, title_bar::TitleBar}, ui::PageType};

pub trait Page: Sized + Render {
    fn controls(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement;
    fn scrollable(&self, cx: &App) -> bool;
}

pub fn page_layout(
    page_type: PageType,
    page_path: Arc<[PageType]>,
    controls: impl IntoElement,
    scrollable: bool,
    content: impl IntoElement,
) -> impl IntoElement {
    v_flex()
        .size_full()
        .child(TitleBar::new(PagePath::new(page_type, page_path), controls.into_any_element()))
        .child(if scrollable {
            div().flex_1().min_h_0().overflow_hidden().child(
                v_flex().size_full().overflow_y_scrollbar().child(content),
            ).into_any_element()
        } else {
            div().flex_1().min_h_0().overflow_hidden().child(content).into_any_element()
        })
}
