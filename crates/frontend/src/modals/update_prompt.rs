use std::{path::Path, sync::Arc};

use bridge::{handle::BackendHandle, instance::InstanceStatus, modal_action::ModalAction};
use gpui::{prelude::*, *};
use gpui_component::{
    Root, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    notification::NotificationType,
    v_flex,
};
use schema::pandora_update::UpdatePrompt;

use crate::{entity::DataEntities, root::LauncherRoot};

/// What the title bar has to offer about updating Pandora itself.
#[derive(Clone)]
pub enum PendingUpdate {
    /// A newer version can be downloaded.
    Available(UpdatePrompt),
    /// A newer version is in place and runs once the launcher is started again.
    Restart(PendingRestart),
}

#[derive(Clone)]
pub struct PendingRestart {
    /// What to run to start the new version.
    pub relaunch: Arc<Path>,
    /// The version, when the launcher installed it itself rather than finding itself replaced.
    pub version: Option<Arc<str>>,
}

/// Shows `update` in the title bar of the main window, `window`.
pub fn set_pending_update(update: PendingUpdate, window: &mut Window, cx: &mut App) {
    let Some(root) = window.root::<Root>().flatten() else {
        return;
    };
    let Ok(launcher_root) = root.read(cx).view().clone().downcast::<LauncherRoot>() else {
        return;
    };

    launcher_root.update(cx, |launcher_root, cx| {
        launcher_root.ui.update(cx, |ui, cx| {
            ui.update = Some(update);
            cx.notify();
        });
    });
}

pub fn open_update_prompt(update: UpdatePrompt, handle: BackendHandle, window: &mut Window, cx: &mut App) {
    let title = t::system::update::title();
    let old_version: SharedString = t::system::update::current(&update.old_version).into();
    let new_version: SharedString = t::system::update::new(&update.new_version).into();

    let size = if update.exe.size < 1000 * 10 {
        t::system::update::size::bytes(update.exe.size)
    } else if update.exe.size < 1000 * 1000 * 10 {
        t::system::update::size::kb(update.exe.size / 1000)
    } else if update.exe.size < 1000 * 1000 * 1000 * 10 {
        t::system::update::size::mb(update.exe.size / 1000 / 1000)
    } else {
        t::system::update::size::gb(update.exe.size / 1000 / 1000 / 1000)
    };

    let size = SharedString::from(size);

    window.open_dialog(cx, move |dialog, _, _| {
        let buttons = h_flex()
            .w_full()
            .gap_2()
            .child(Button::new("update").flex_1().label(t::common::update()).success().on_click({
                let handle = handle.clone();
                let update = update.clone();
                move |_, window, cx| {
                    let modal_action = ModalAction::default();
                    handle.send(bridge::message::MessageToBackend::InstallUpdate {
                        update: update.clone(),
                        modal_action: modal_action.clone(),
                    });
                    window.close_all_dialogs(cx);
                    crate::modals::generic::show_notification(
                        window,
                        cx,
                        t::system::update::install_error().into(),
                        modal_action,
                    );
                }
            }))
            .child(Button::new("later").flex_1().label(t::system::update::later()).on_click(|_, window, cx| {
                window.close_all_dialogs(cx);
            }));

        dialog.title(title).overlay_closable(false).child(
            v_flex()
                .gap_2()
                .child(v_flex().child(old_version.clone()).child(new_version.clone()).child(size.clone()))
                .child(buttons),
        )
    });
}

/// Asks whether to restart into a new version that's in place, and keeps offering it in the
/// title bar if the answer is later.
pub fn offer_restart(restart: PendingRestart, data: &DataEntities, window: &mut Window, cx: &mut App) {
    set_pending_update(PendingUpdate::Restart(restart.clone()), window, cx);
    open_restart_prompt(restart, data.clone(), window, cx);
}

pub fn open_restart_prompt(restart: PendingRestart, data: DataEntities, window: &mut Window, cx: &mut App) {
    let message: SharedString = match &restart.version {
        Some(version) => t::system::update::restart::installed(version).into(),
        None => t::system::update::restart::replaced().into(),
    };

    window.open_dialog(cx, move |dialog, _, _| {
        let buttons = h_flex()
            .w_full()
            .gap_2()
            .child(
                Button::new("restart")
                    .flex_1()
                    .label(t::system::update::restart::now())
                    .success()
                    .on_click({
                        let relaunch = restart.relaunch.clone();
                        let data = data.clone();
                        move |_, window, cx| restart_launcher(&relaunch, &data, window, cx)
                    }),
            )
            .child(Button::new("later").flex_1().label(t::system::update::later()).on_click(|_, window, cx| {
                window.close_all_dialogs(cx);
            }));

        dialog
            .title(t::system::update::restart::title())
            .overlay_closable(false)
            .child(v_flex().gap_2().child(message.clone()).child(buttons))
    });
}

/// Starts the launcher at `relaunch` and quits this one, which the new one waits for.
fn restart_launcher(relaunch: &Path, data: &DataEntities, window: &mut Window, cx: &mut App) {
    // The launcher can't exit while it's looking after a game or server
    let instances = data.instances.read(cx);
    if instances
        .entries
        .values()
        .any(|instance| instance.read(cx).status != InstanceStatus::NotRunning)
    {
        window.push_notification((NotificationType::Error, t::system::update::restart::instances_running()), cx);
        return;
    }

    if let Err(err) = bridge::launcher::spawn_restart(relaunch) {
        log::error!("Unable to start {:?} to restart into: {}", relaunch, err);
        let message = SharedString::from(t::system::update::restart::error(err));
        window.push_notification((NotificationType::Error, message), cx);
        return;
    }

    window.close_all_dialogs(cx);
    cx.defer(crate::quit);
}
