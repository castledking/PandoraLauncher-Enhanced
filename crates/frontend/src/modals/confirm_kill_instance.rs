use bridge::{handle::BackendHandle, instance::InstanceID};
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex, v_flex, WindowExt,
};

pub fn open_confirm_kill_instance(
    instance: InstanceID,
    instance_name: SharedString,
    backend_handle: BackendHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let title = SharedString::new(format!("Kill Instance: {}", instance_name));
    let message = SharedString::new(format!(
        "The instance '{}' is already running. Would you like to kill it and start a new session?",
        instance_name
    ));

    window.open_dialog(cx, move |dialog, _, _| {
        let title = title.clone();
        let message = message.clone();
        let buttons = h_flex()
            .w_full()
            .gap_2()
            .child(Button::new("cancel").flex_1().label("Cancel").on_click(|_, window, cx| {
                window.close_all_dialogs(cx);
            }))
            .child(Button::new("kill").flex_1().danger().label("Kill Instance").on_click({
                let backend_handle = backend_handle.clone();
                move |_, window, cx| {
                    backend_handle.send(bridge::message::MessageToBackend::KillInstance { id: instance });
                    window.close_all_dialogs(cx);
                }
            }));

        dialog.title(title).child(v_flex().gap_4().child(message).child(buttons))
    });
}
