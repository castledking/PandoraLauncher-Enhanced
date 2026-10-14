//! Dialogs for content labels: picking a highlight colour by HEX, adding a category, and managing
//! the list of categories.

use std::{rc::Rc, sync::Arc};

use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Icon, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    dialog::Dialog,
    h_flex,
    input::{Input, InputEvent, InputState},
    switch::Switch,
    tooltip::Tooltip,
    v_flex,
};
use schema::content_label::HexColor;

use crate::{
    component::content_label::{
        category_chip, category_exists, category_icon, category_name, preset_colors, row_tint, swatch, to_hsla,
    },
    icon::PandoraIcon,
    interface_config::{ContentCategory, InterfaceConfig, default_content_categories},
};

/// A colour typed into a HEX box: nothing typed, a valid colour, or something that isn't one.
#[derive(Clone, Copy, PartialEq)]
enum HexEntry {
    Empty,
    Valid(HexColor),
    Invalid,
}

impl HexEntry {
    fn read(input: &Entity<InputState>, cx: &App) -> Self {
        let text = input.read(cx).value();
        if text.trim().is_empty() {
            HexEntry::Empty
        } else {
            HexColor::parse(&text).map(HexEntry::Valid).unwrap_or(HexEntry::Invalid)
        }
    }

    fn color(self) -> Option<HexColor> {
        match self {
            HexEntry::Valid(color) => Some(color),
            HexEntry::Empty | HexEntry::Invalid => None,
        }
    }
}

/// The preset colours as a row of round swatches, the current one ringed.
fn render_swatches(
    selected: Option<HexColor>,
    on_pick: impl Fn(HexColor, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let on_pick = Rc::new(on_pick);
    h_flex().gap_2().flex_wrap().children(preset_colors().map(|(color, name)| {
        let on_pick = on_pick.clone();
        let ring = if selected == Some(color) {
            cx.theme().foreground
        } else {
            cx.theme().transparent
        };
        div()
            .id(("swatch", color.rgb() as usize))
            .size_7()
            .flex_none()
            .rounded_full()
            .border_2()
            .border_color(ring)
            .p_0p5()
            .cursor_pointer()
            .child(div().size_full().rounded_full().bg(to_hsla(color)))
            .tooltip(move |window, cx| Tooltip::new(name).build(window, cx))
            .on_click(move |_, window, cx| on_pick(color, window, cx))
    }))
}

/// The HEX box, a swatch of what it holds, and a hint when what's typed isn't a colour.
fn render_hex_field(input: &Entity<InputState>, entry: HexEntry, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_sm().child(t::instance::content::label::color_dialog::hex()))
        .child(
            h_flex()
                .gap_2()
                .child(div().flex_1().child(Input::new(input).small()))
                .child(swatch(entry.color(), cx).size_6()),
        )
        .when(entry == HexEntry::Invalid, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(t::instance::content::label::color_dialog::invalid()),
            )
        })
}

/// A row as the content list would draw it with `color`, so the tint can be judged before it's
/// applied.
fn render_row_preview(name: SharedString, color: Option<HexColor>, cx: &App) -> impl IntoElement {
    let row = h_flex()
        .gap_2()
        .p_1()
        .border_1()
        .rounded(cx.theme().radius)
        .border_color(cx.theme().transparent)
        .child(Switch::new("preview-toggle").checked(true).small().px_2())
        .child(div().flex_1().min_w_0().overflow_x_hidden().whitespace_nowrap().text_sm().child(name));

    let row = match color {
        Some(color) => {
            let (background, border) = row_tint(color, false, cx);
            row.bg(background).border_color(border)
        },
        None => row,
    };

    v_flex()
        .gap_1()
        .child(div().text_sm().child(t::instance::content::label::color_dialog::preview()))
        .child(row)
}

struct ColorPickerDialog {
    hex_input: Entity<InputState>,
    entry: HexEntry,
    preview_name: SharedString,
    can_remove: bool,
    on_apply: Rc<dyn Fn(Option<HexColor>, &mut Window, &mut App)>,
    _hex_subscription: Subscription,
}

impl ColorPickerDialog {
    fn new(
        initial: Option<HexColor>,
        preview_name: SharedString,
        on_apply: Rc<dyn Fn(Option<HexColor>, &mut Window, &mut App)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hex_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("#3b82f6");
            if let Some(initial) = initial {
                state.set_value(initial.to_string(), window, cx);
            }
            state
        });

        let _hex_subscription =
            cx.subscribe_in(&hex_input, window, |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.entry = HexEntry::read(input, cx);
                    cx.notify();
                },
                InputEvent::PressEnter { .. } => this.apply(this.entry.color(), window, cx),
                _ => {},
            });

        Self {
            hex_input,
            entry: initial.map(HexEntry::Valid).unwrap_or(HexEntry::Empty),
            preview_name,
            can_remove: initial.is_some(),
            on_apply,
            _hex_subscription,
        }
    }

    fn pick(&mut self, color: HexColor, window: &mut Window, cx: &mut Context<Self>) {
        // set_value doesn't report a change, so the entry is updated here too
        self.hex_input.update(cx, |state, cx| state.set_value(color.to_string(), window, cx));
        self.entry = HexEntry::Valid(color);
        cx.notify();
    }

    fn apply(&mut self, color: Option<HexColor>, window: &mut Window, cx: &mut Context<Self>) {
        if self.entry == HexEntry::Invalid && color.is_some() {
            return;
        }
        (self.on_apply)(color, window, cx);
        window.close_dialog(cx);
    }

    fn render(&mut self, dialog: Dialog, _window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        let entity = cx.entity();
        let entry = self.entry;

        let swatches = render_swatches(
            entry.color(),
            move |color, window, cx| entity.update(cx, |this, cx| this.pick(color, window, cx)),
            cx,
        );

        let footer = h_flex()
            .gap_2()
            .when(self.can_remove, |this| {
                this.child(
                    Button::new("remove")
                        .label(t::instance::content::label::color_dialog::remove())
                        .danger()
                        .on_click(cx.listener(|this, _, window, cx| this.apply(None, window, cx))),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("cancel")
                    .label(t::common::cancel())
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
            .child(
                Button::new("apply")
                    .label(t::instance::content::label::color_dialog::apply())
                    .success()
                    .disabled(entry == HexEntry::Invalid)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let color = this.entry.color();
                        this.apply(color, window, cx)
                    })),
            );

        dialog.overlay_closable(true).child(
            v_flex()
                .gap_4()
                .child(swatches)
                .child(render_hex_field(&self.hex_input, entry, cx))
                .child(render_row_preview(self.preview_name.clone(), entry.color(), cx))
                .child(footer),
        )
    }
}

/// Asks for a colour, either one of the presets or any typed in as HEX, and hands it to
/// `on_apply`; `None` means the colour was removed.
pub fn open_color_picker(
    title: SharedString,
    initial: Option<HexColor>,
    preview_name: SharedString,
    on_apply: Rc<dyn Fn(Option<HexColor>, &mut Window, &mut App)>,
    window: &mut Window,
    cx: &mut App,
) {
    let state = cx.new(|cx| ColorPickerDialog::new(initial, preview_name, on_apply, window, cx));
    let hex_input = state.read(cx).hex_input.clone();

    window.open_dialog(cx, move |dialog, window, cx| {
        cx.update_entity(&state, |state, cx| state.render(dialog.title(title.clone()), window, cx))
    });
    hex_input.update(cx, |input, cx| input.focus(window, cx));
}

struct NewCategoryDialog {
    name_input: Entity<InputState>,
    hex_input: Entity<InputState>,
    entry: HexEntry,
    error: Option<SharedString>,
    on_created: Option<Rc<dyn Fn(Arc<str>, &mut Window, &mut App)>>,
    _subscriptions: [Subscription; 2],
}

impl NewCategoryDialog {
    fn new(
        on_created: Option<Rc<dyn Fn(Arc<str>, &mut Window, &mut App)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Start on a colour no category has yet, so a new one stands apart from the rest
        let taken: Vec<HexColor> = InterfaceConfig::get(cx)
            .content_categories
            .iter()
            .filter_map(|category| category.color)
            .collect();
        let initial = preset_colors()
            .map(|(color, _)| color)
            .find(|color| !taken.contains(color))
            .or_else(|| preset_colors().next().map(|(color, _)| color));

        let name_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(t::instance::content::label::manage::name_placeholder()));
        let hex_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("#3b82f6");
            if let Some(initial) = initial {
                state.set_value(initial.to_string(), window, cx);
            }
            state
        });

        let name_subscription =
            cx.subscribe_in(&name_input, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.error = None;
                    cx.notify();
                },
                InputEvent::PressEnter { .. } => this.create(window, cx),
                _ => {},
            });
        let hex_subscription =
            cx.subscribe_in(&hex_input, window, |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.entry = HexEntry::read(input, cx);
                    cx.notify();
                },
                InputEvent::PressEnter { .. } => this.create(window, cx),
                _ => {},
            });

        Self {
            name_input,
            hex_input,
            entry: initial.map(HexEntry::Valid).unwrap_or(HexEntry::Empty),
            error: None,
            on_created,
            _subscriptions: [name_subscription, hex_subscription],
        }
    }

    fn pick(&mut self, color: HexColor, window: &mut Window, cx: &mut Context<Self>) {
        self.hex_input.update(cx, |state, cx| state.set_value(color.to_string(), window, cx));
        self.entry = HexEntry::Valid(color);
        cx.notify();
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.entry == HexEntry::Invalid {
            return;
        }

        let name = self.name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error = Some(t::instance::content::label::manage::empty_name().into());
            cx.notify();
            return;
        }
        if category_exists(&name, &InterfaceConfig::get(cx).content_categories) {
            self.error = Some(t::instance::content::label::manage::duplicate(&name).into());
            cx.notify();
            return;
        }

        let id: Arc<str> = name.into();
        InterfaceConfig::get_mut(cx).content_categories.push(ContentCategory {
            id: id.clone(),
            color: self.entry.color(),
        });
        if let Some(on_created) = &self.on_created {
            on_created(id, window, cx);
        }
        window.close_dialog(cx);
        cx.refresh_windows();
    }

    fn render(&mut self, dialog: Dialog, _window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        let entity = cx.entity();
        let entry = self.entry;

        let name = self.name_input.read(cx).value().trim().to_string();
        let preview = (!name.is_empty()).then(|| category_chip(&name, entry.color(), cx));

        let swatches = render_swatches(
            entry.color(),
            move |color, window, cx| entity.update(cx, |this, cx| this.pick(color, window, cx)),
            cx,
        );

        let content = v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child(t::instance::content::label::manage::name()))
                    .child(Input::new(&self.name_input).small())
                    .when_some(self.error.clone(), |this, error| {
                        this.child(div().text_xs().text_color(cx.theme().danger).child(error))
                    }),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child(t::instance::content::label::manage::color()))
                    .child(swatches),
            )
            .child(render_hex_field(&self.hex_input, entry, cx))
            .when_some(preview, |this, preview| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(div().text_sm().child(t::instance::content::label::color_dialog::preview()))
                        .child(h_flex().child(preview)),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("cancel")
                            .label(t::common::cancel())
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("create")
                            .label(t::instance::content::label::manage::create())
                            .success()
                            .disabled(entry == HexEntry::Invalid)
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                    ),
            );

        dialog
            .overlay_closable(true)
            .title(t::instance::content::label::manage::new_title())
            .child(content)
    }
}

/// Asks for a new category's name and colour and adds it to the list. `on_created` is given the
/// new category's id, to put whatever the dialog was opened for straight into it.
pub fn open_new_category(
    on_created: Option<Rc<dyn Fn(Arc<str>, &mut Window, &mut App)>>,
    window: &mut Window,
    cx: &mut App,
) {
    let state = cx.new(|cx| NewCategoryDialog::new(on_created, window, cx));
    let name_input = state.read(cx).name_input.clone();

    window.open_dialog(cx, move |dialog, window, cx| {
        cx.update_entity(&state, |state, cx| state.render(dialog, window, cx))
    });
    name_input.update(cx, |input, cx| input.focus(window, cx));
}

fn render_category_row(index: usize, category: &ContentCategory, cx: &App) -> impl IntoElement {
    let id = category.id.clone();
    let name = category_name(&id);

    let set_color = {
        let id = id.clone();
        let name = name.clone();
        let color = category.color;
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            let id = id.clone();
            let on_apply: Rc<dyn Fn(Option<HexColor>, &mut Window, &mut App)> = Rc::new(move |color, _, cx| {
                let config = InterfaceConfig::get_mut(cx);
                if let Some(category) = config.content_categories.iter_mut().find(|category| category.id == id) {
                    category.color = color;
                }
                cx.refresh_windows();
            });
            open_color_picker(
                t::instance::content::label::manage::color_title(&name).into(),
                color,
                name.clone(),
                on_apply,
                window,
                cx,
            );
        }
    };

    let delete = move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        InterfaceConfig::get_mut(cx).content_categories.retain(|category| category.id != id);
        cx.refresh_windows();
    };

    h_flex()
        .gap_2()
        .px_2()
        .py_1()
        .rounded(cx.theme().radius)
        .hover(|this| this.bg(cx.theme().list_hover))
        .child(
            div()
                .w_4()
                .flex_none()
                .when_some(category_icon(&category.id), |this, icon| this.child(Icon::empty().path(icon).size_4())),
        )
        .child(div().flex_1().min_w_0().overflow_x_hidden().whitespace_nowrap().child(name))
        .child(
            Button::new(("category-color", index))
                .small()
                .outline()
                .child(swatch(category.color, cx))
                .tooltip(t::instance::content::label::manage::set_color())
                .on_click(set_color),
        )
        .child(
            Button::new(("category-delete", index))
                .small()
                .danger()
                .icon(PandoraIcon::Trash2)
                .tooltip(t::instance::content::label::manage::delete())
                .on_click(delete),
        )
}

/// Lists the categories with a way to recolour, delete and add them. Changes apply at once, since
/// the list is shared and every content list follows it.
pub fn open_manage_categories(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let categories = InterfaceConfig::get(cx).content_categories.clone();

        let list = if categories.is_empty() {
            div()
                .py_4()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t::instance::content::label::manage::empty())
                .into_any_element()
        } else {
            v_flex()
                .id("categories")
                .max_h_96()
                .overflow_y_scroll()
                .children(
                    categories
                        .iter()
                        .enumerate()
                        .map(|(index, category)| render_category_row(index, category, cx)),
                )
                .into_any_element()
        };

        let footer = h_flex()
            .gap_2()
            .child(
                Button::new("add-category")
                    .label(t::instance::content::label::manage::add())
                    .icon(PandoraIcon::Plus)
                    .success()
                    .on_click(|_, window, cx| open_new_category(None, window, cx)),
            )
            .child(
                Button::new("restore-defaults")
                    .label(t::instance::content::label::manage::restore_defaults())
                    .outline()
                    .on_click(|_, _, cx| {
                        // Puts back the built-in categories as they came, keeping the user's own
                        let config = InterfaceConfig::get_mut(cx);
                        let defaults = default_content_categories();
                        let own = config
                            .content_categories
                            .iter()
                            .filter(|category| !defaults.iter().any(|default| default.id == category.id))
                            .cloned()
                            .collect::<Vec<_>>();
                        config.content_categories = defaults.into_iter().chain(own).collect();
                        cx.refresh_windows();
                    }),
            )
            .child(div().flex_1())
            .child(
                Button::new("done")
                    .label(t::instance::content::label::manage::done())
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            );

        dialog.overlay_closable(true).title(t::instance::content::categories()).child(
            v_flex()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t::instance::content::label::manage::description()),
                )
                .child(list)
                .child(footer),
        )
    });
}
