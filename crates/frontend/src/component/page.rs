use gpui::{prelude::FluentBuilder, *};
use gpui_component::{ActiveTheme, Colorize, h_flex, scroll::ScrollableElement, v_flex};

use crate::icon::PandoraIcon;

#[derive(IntoElement)]
pub struct Page {
    title: AnyElement,
    scrollable: bool,
    children: Vec<AnyElement>,
}

impl Page {
    pub fn new(title: impl IntoElement) -> Self {
        Self {
            title: title.into_any_element(),
            scrollable: false,
            children: Vec::new(),
        }
    }

    pub fn scrollable(mut self) -> Self {
        self.scrollable = true;
        self
    }
}

impl ParentElement for Page {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

#[derive(Default)]
struct TitleBarState {
    should_move: bool,
}

impl RenderOnce for Page {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let state = window.use_keyed_state("title-bar-state", cx, |_, _| TitleBarState::default());

        let window_controls = window.window_controls();

        let title = h_flex()
            .id("bar")
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down_out(window.listener_for(&state, |state, _, _, _| {
                state.should_move = false;
            }))
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(&state, |state, _, _, _| {
                    state.should_move = true;
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                window.listener_for(&state, |state, _, _, _| {
                    state.should_move = false;
                }),
            )
            .on_mouse_move(window.listener_for(&state, |state, _, window, _| {
                if state.should_move {
                    state.should_move = false;
                    window.start_window_move();
                }
            }))
            .w_full()
            .min_h(px(57.0))
            .max_h(px(57.0))
            .h(px(57.0))
            .p_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_xl()
            .child(div().left_2().child(self.title))
            .child(h_flex().absolute().right_0().pr_4()
                .gap_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .bg(cx.theme().background)
                .h_full()
                .when(window_controls.minimize, |this| this.child(WindowControl::Minimize))
                .when(window_controls.maximize, |this| this.child(WindowControl::Maximize))
                .child(WindowControl::Close));

        if self.scrollable {
            v_flex()
                .size_full()
                .child(title)
                .child(div().flex_1().overflow_hidden().child(
                    v_flex().size_full().overflow_y_scrollbar().children(self.children),
                ))
        } else {
            v_flex()
                .size_full()
                .child(title)
                .children(self.children)
        }
    }
}

#[derive(IntoElement, Clone, Copy, PartialEq, Eq)]
pub enum WindowControl {
    Minimize,
    Maximize,
    Close,
}

impl RenderOnce for WindowControl {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .id(match self {
                WindowControl::Minimize => "minimize",
                WindowControl::Maximize => "maximize",
                WindowControl::Close => "close",
            })
            .window_control_area(match self {
                WindowControl::Minimize => WindowControlArea::Min,
                WindowControl::Maximize => WindowControlArea::Max,
                WindowControl::Close => WindowControlArea::Close,
            })
            .p_1()
            .rounded(cx.theme().radius)
            .hover(|this| {
                let col = if self == WindowControl::Close {
                    cx.theme().danger_hover
                } else if cx.theme().mode.is_dark() {
                    cx.theme().secondary.lighten(0.1).opacity(0.8)
                } else {
                    cx.theme().secondary.darken(0.1).opacity(0.8)
                };
                this.bg(col)
            })
            .on_click(move |_, window, _| {
                match self {
                    WindowControl::Minimize => window.minimize_window(),
                    WindowControl::Maximize => window.zoom_window(),
                    WindowControl::Close => window.remove_window(),
                }
            })
            .child(match self {
                WindowControl::Minimize => PandoraIcon::WindowMinimize,
                WindowControl::Maximize => PandoraIcon::WindowMaximize,
                WindowControl::Close => PandoraIcon::WindowClose,
            })
    }
}
