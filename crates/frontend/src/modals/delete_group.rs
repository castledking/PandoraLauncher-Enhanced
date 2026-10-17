use bridge::{handle::BackendHandle, instance::InstanceID, message::MessageToBackend};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, WindowExt,
    button::{Button, ButtonVariants},
    v_flex,
};

use crate::interface_config::InterfaceConfig;

/// Confirms deleting every instance in a group, naming each one so nothing goes by surprise.
pub fn open_delete_group(
    group_name: SharedString,
    members: Vec<(InstanceID, SharedString)>,
    backend_handle: BackendHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let title: SharedString = t::instance::group::delete_dialog::title(&group_name).into();
    let confirm: SharedString = t::instance::group::delete_dialog::confirm(members.len()).into();

    window.open_dialog(cx, move |dialog, _, cx| {
        let names = v_flex()
            .gap_1()
            .pl_2()
            .text_color(cx.theme().muted_foreground)
            .children(members.iter().map(|(_, name)| format!("• {name}")));

        let delete = Button::new("delete-group").danger().label(confirm.clone()).on_click({
            let members = members.clone();
            let backend_handle = backend_handle.clone();
            let group_name = group_name.clone();
            move |_, window, cx| {
                for (id, _) in &members {
                    backend_handle.send(MessageToBackend::DeleteInstance { id: *id });
                }
                forget_group(&group_name, cx);
                window.close_all_dialogs(cx);
            }
        });

        dialog.title(title.clone()).child(
            v_flex()
                .gap_3()
                .child(t::instance::group::delete_dialog::warning())
                .child(names)
                .child(delete),
        )
    });
}

/// Drops a group's ordering and collapsed state, so a group made later under the same name starts
/// out fresh rather than inheriting them.
pub fn forget_group(group_name: &SharedString, cx: &mut App) {
    let config = InterfaceConfig::get_mut(cx);
    config.instance_group_order.retain(|name| **name != **group_name);
    config.instance_groups_closed.remove(group_name);
}
