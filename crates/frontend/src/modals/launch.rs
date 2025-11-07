use std::sync::Arc;

use bridge::modal_action::ModalAction;
use gpui::{prelude::*, *};
use gpui_component::{button::{Button, ButtonVariants}, v_flex, WindowExt};

use crate::component::{error_alert::ErrorAlert, progress_bar::{ProgressBar, ProgressBarColor}};

pub fn show_launching_modal(window: &mut Window, cx: &mut App, name: SharedString, modal_action: ModalAction) {
    let title: SharedString = format!("Launching {}", name).into();
    
    window.open_modal(cx, move |modal, window, cx| {
        if let Some(error) = &*modal_action.error.read().unwrap() {
            let error_widget = ErrorAlert::new(
                "error",
                "Error starting instance".into(),
                error.clone().into()
            );

            return modal
                .confirm()
                .title(title.clone())
                .child(v_flex().gap_3().child(error_widget));
        }
        
        let mut modal_opacity = 1.0;
        if let Some(finished_at) = modal_action.get_finished_at() {
            let elapsed = finished_at.elapsed().as_secs_f32();
            window.request_animation_frame();
            if elapsed >= 2.0 {
                window.close_modal(cx);
                return modal.opacity(0.0);
            } else if elapsed >= 1.0 {
                modal_opacity = 2.0 - elapsed;
            }
        }
        
        let trackers = modal_action.trackers.trackers.read().unwrap();
        let mut progress_entries = Vec::with_capacity(trackers.len());
        for tracker in &*trackers {
            let mut opacity = 1.0;

            let mut progress_bar = ProgressBar::new();
            if let Some(progress_amount) = tracker.get_float() {
                progress_bar.amount = progress_amount;
            }

            if let Some(finished_at) = tracker.get_finished_at() {
                let elapsed = finished_at.elapsed().as_secs_f32();
                if elapsed >= 2.0 {
                    continue;
                } else if elapsed >= 1.0 {
                    opacity = 2.0 - elapsed;
                }

                if tracker.is_error() {
                    progress_bar.color = ProgressBarColor::Error;
                } else {
                    progress_bar.color = ProgressBarColor::Success;
                }
                if elapsed <= 0.5 {
                    progress_bar.color_scale = elapsed * 2.0;
                }

                window.request_animation_frame();
            }

            let title = tracker.get_title();
            progress_entries.push(div().gap_3().child(SharedString::from(title)).child(progress_bar).opacity(opacity));
        }
        drop(trackers);
        
        if let Some(visit_url) = &*modal_action.visit_url.read().unwrap() {
            let message = SharedString::new(Arc::clone(&visit_url.message));
            let url = Arc::clone(&visit_url.url);
            progress_entries.push(div().p_3().child(Button::new("visit").success().label(message).on_click(move |_, _, cx| {
                cx.open_url(&url);
            })));
        }

        let progress = v_flex().gap_2().children(progress_entries);

        modal.title(title.clone()).child(progress).opacity(modal_opacity)
    });
}
