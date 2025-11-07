use std::sync::{Arc, RwLock};

use bridge::{handle::BackendHandle, instance::InstanceID, message::{MessageToBackend, QuickPlayLaunch}, modal_action::ModalAction};
use gpui::{prelude::*, *};
use gpui_component::{v_flex, Root};

use crate::{entity::DataEntities, modals, ui::LauncherUI};

pub struct LauncherRootGlobal {
    pub root: Entity<LauncherRoot>,
}

impl Global for LauncherRootGlobal {}

pub struct LauncherRoot {
    pub ui: Entity<LauncherUI>,
    pub panic_message: Arc<RwLock<Option<String>>>,
    pub backend_handle: BackendHandle,
}

impl LauncherRoot {
    pub fn new(data: &DataEntities, panic_message: Arc<RwLock<Option<String>>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let launcher_ui = cx.new(|cx| LauncherUI::new(&data, window, cx));
        
        Self {
            ui: launcher_ui,
            panic_message,
            backend_handle: data.backend_handle.clone()
        }
    }
}

impl Render for LauncherRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sheet_layer = Root::render_sheet_layer(window, cx);
        let modal_layer = Root::render_modal_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        if let Some(message) = &*self.panic_message.read().unwrap() {
            return v_flex().size_full().bg(gpui::blue()).child(message.clone());
        }
        if self.backend_handle.is_closed() {
            return v_flex().size_full().bg(gpui::red()).child("Backend has abruptly shutdown");
        }
        
        div()
            .size_full()
            .font_family("Inter 24pt")
            .child(self.ui.clone())
            .children(sheet_layer)
            .children(modal_layer)
            .children(notification_layer)
    }
}

pub fn start_instance(id: InstanceID, name: SharedString, quick_play: Option<QuickPlayLaunch>, backend_handle: &BackendHandle, window: &mut Window, cx: &mut App) {
    let modal_action = ModalAction::default();
    
    backend_handle.blocking_send(MessageToBackend::StartInstance {
        id,
        quick_play,
        modal_action: modal_action.clone(),
    });
    
    modals::launch::show_launching_modal(window, cx, name, modal_action);
}
