use std::sync::Arc;

use bridge::{handle::BackendHandle, message::MessageToBackend};
use gpui::{prelude::*, *};
use gpui_component::{
    WindowExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

pub fn open_delete_skin(
    skin_id: Arc<str>,
    backend_handle: BackendHandle,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Delete Skin").child(
            v_flex()
                .gap_3()
                .child("Are you sure you want to delete this skin?")
                .child("It will be sent to your trash.")
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_all_dialogs(cx)),
                        )
                        .child(
                            Button::new("delete")
                                .label("Delete")
                                .danger()
                                .on_click({
                                    let backend_handle = backend_handle.clone();
                                    let skin_id = skin_id.clone();
                                    move |_, window, cx| {
                                        backend_handle.send(MessageToBackend::DeleteOwnedSkin {
                                            skin_id: skin_id.clone(),
                                        });
                                        window.close_all_dialogs(cx);
                                    }
                                }),
                        ),
                ),
        )
    });
}
