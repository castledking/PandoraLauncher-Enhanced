use gpui::{prelude::*, *};
use gpui_component::{
    WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    notification::NotificationType,
    v_flex,
};

pub fn open_rename_group(
    title: &'static str,
    current_name: String,
    on_confirm: impl Fn(String, &mut Window, &mut App) + Clone + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let input_state = cx.new(|cx| InputState::new(window, cx));
    input_state.update(cx, |state, cx| {
        state.set_value(current_name.clone(), window, cx);
    });

    window.open_dialog(cx, move |dialog, _, _| {
        let content = v_flex().gap_4().child(Input::new(&input_state)).child(
            h_flex()
                .gap_2()
                .justify_end()
                .child(Button::new("cancel").label(t::common::cancel()).on_click({
                    move |_, window, cx| {
                        window.close_dialog(cx);
                    }
                }))
                .child(Button::new("rename").label(t::instance::group::rename()).success().on_click({
                    let input_state = input_state.clone();
                    let on_confirm = on_confirm.clone();
                    move |_, window, cx| {
                        let new_name = input_state.read(cx).value().trim().to_string();
                        if new_name.is_empty() {
                            window.push_notification((NotificationType::Error, "Group name cannot be empty"), cx);
                            return;
                        }
                        on_confirm(new_name, window, cx);
                        window.close_dialog(cx);
                    }
                })),
        );

        dialog.title(title).child(content)
    });
}
