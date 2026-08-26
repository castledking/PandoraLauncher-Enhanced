use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    v_flex,
};

use crate::interface_config::InterfaceConfig;

#[derive(Debug, Clone)]
pub struct SelectedGroup {
    /// `None` means ungrouped.
    pub id: Option<u64>,
    pub name: SharedString,
}

#[derive(Debug, Clone)]
pub enum GroupSelection {
    Existing(SelectedGroup),
    New(String),
}

pub fn open_select_group(
    current: Option<SelectedGroup>,
    on_select: impl Fn(GroupSelection, &mut Window, &mut App) + Clone + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let input_state = cx.new(|cx| InputState::new(window, cx).placeholder(t::instance::group::new_placeholder()));
    let theme = cx.theme().clone();

    window.open_dialog(cx, move |dialog, _, cx| {
        let groups = InterfaceConfig::get(cx).instance_groups.clone();
        let assignments = InterfaceConfig::get(cx).instance_group_assignments.clone();

        let mut list = v_flex().gap_1();

        let ungrouped_selected = current.as_ref().is_some_and(|g| g.id.is_none());
        let ungrouped_entry = h_flex()
            .id("group-ungrouped")
            .justify_between()
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .when(ungrouped_selected, |this| this.bg(theme.secondary))
            .hover(|this| this.bg(theme.secondary))
            .on_click({
                let on_select = on_select.clone();
                move |_, window, cx| {
                    on_select(
                        GroupSelection::Existing(SelectedGroup {
                            id: None,
                            name: SharedString::from(t::instance::group::ungrouped()),
                        }),
                        window,
                        cx,
                    );
                    window.close_dialog(cx);
                }
            })
            .child(t::instance::group::ungrouped())
            .when(ungrouped_selected, |this| {
                this.child(Icon::default().path("icons/check.svg").text_color(theme.link))
            });
        list = list.child(ungrouped_entry);

        for group in &groups {
            let count = assignments.iter().filter(|(_, assigned)| *assigned == group.id).count();
            let selected = current.as_ref().is_some_and(|g| g.id == Some(group.id));
            let entry = h_flex()
                .id(("group-entry", group.id))
                .justify_between()
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .when(selected, |this| this.bg(theme.secondary))
                .hover(|this| this.bg(theme.secondary))
                .on_click({
                    let on_select = on_select.clone();
                    let name = group.name.clone();
                    let group_id = group.id;
                    move |_, window, cx| {
                        on_select(
                            GroupSelection::Existing(SelectedGroup {
                                id: Some(group_id),
                                name: name.clone().into(),
                            }),
                            window,
                            cx,
                        );
                        window.close_dialog(cx);
                    }
                })
                .child(group.name.clone())
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_color(theme.secondary_foreground)
                                .text_sm()
                                .child(t::instance::group::instances_count(count)),
                        )
                        .when(selected, |this| {
                            this.child(Icon::default().path("icons/check.svg").text_color(theme.link))
                        }),
                );
            list = list.child(entry);
        }

        let content = v_flex().gap_4().child(list).child(
            h_flex().gap_2().child(Input::new(&input_state)).child(
                Button::new("create_group")
                    .label(t::instance::group::new())
                    .success()
                    .compact()
                    .icon(Icon::default().path("icons/plus.svg"))
                    .on_click({
                        let on_select = on_select.clone();
                        let input_state = input_state.clone();
                        move |_, window, cx| {
                            let name = input_state.read(cx).value().trim().to_string();
                            if !name.is_empty() {
                                on_select(GroupSelection::New(name), window, cx);
                                window.close_dialog(cx);
                            }
                        }
                    }),
            ),
        );

        dialog.title(t::instance::group::select()).child(content)
    });
}
