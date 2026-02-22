use std::sync::Arc;

use bridge::{handle::BackendHandle, instance::InstanceID};
use gpui::{Styled, prelude::*, *};
use gpui_component::{
    StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    v_flex,
};

pub fn open_rename_instance(
    instance: InstanceID,
    instance_name: SharedString,
    backend_handle: BackendHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let input_state = cx.new(|cx| InputState::new(window, cx));
    let input_state_clone = input_state.clone();
    input_state.update(cx, |state, cx| {
        state.set_value(instance_name.clone(), window, cx);
    });

    let title = SharedString::new("Rename Instance");
    let current_name = instance_name.clone();

    window.open_dialog(cx, move |dialog, _, _| {
        let content = v_flex()
            .gap_4()
            .child(div().text_xl().font_bold().child(format!("Rename \"{}\"", current_name.clone())))
            .child(Input::new(&input_state_clone))
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("cancel").label("Cancel").on_click({
                        move |_, window, cx| {
                            window.close_dialog(cx);
                        }
                    }))
                    .child(Button::new("rename").label("Rename").success().on_click({
                        let backend_handle = backend_handle.clone();
                        let input_state = input_state_clone.clone();
                        move |_, window, cx| {
                            let new_name = input_state.read(cx).value();
                            if !new_name.is_empty() {
                                backend_handle.send(bridge::message::MessageToBackend::RenameInstance {
                                    id: instance,
                                    name: new_name.as_str().into(),
                                });
                            }
                            window.close_dialog(cx);
                        }
                    })),
            );

        dialog.title(title.clone()).child(content)
    });
}
