use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use bridge::{handle::BackendHandle, instance::InstanceStatus, message::MessageToBackend, modal_action::ModalAction};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, Root, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    dialog::Dialog,
    h_flex,
    notification::NotificationType,
    v_flex,
};
use schema::pandora_update::UpdatePrompt;

use crate::{
    entity::{
        DataEntities,
        instance::{InstanceEntry, InstanceModifiedEvent, InstanceRemovedEvent},
    },
    icon::PandoraIcon,
    png_render_cache,
    root::LauncherRoot,
    ui::PageType,
};

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

/// The update the title bar offers, kept where it can be read while drawing, when the launcher
/// root it's shown in can't be.
struct PendingUpdateGlobal(Option<PendingUpdate>);

impl Global for PendingUpdateGlobal {}

/// Set when someone chooses to update, so the launcher restarts as soon as the update is in place
/// instead of asking a second time.
static RESTART_WHEN_INSTALLED: AtomicBool = AtomicBool::new(false);

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
            // The launcher keeps looking for updates while it runs, and the version it finds is
            // the one that's already installed and waiting for a restart
            if matches!(update, PendingUpdate::Available(_)) && matches!(ui.update, Some(PendingUpdate::Restart(_))) {
                return;
            }
            ui.update = Some(update.clone());
            cx.set_global(PendingUpdateGlobal(Some(update)));
            cx.notify();
        });
    });
}

/// The update the title bar is offering, if any.
pub fn pending_update(cx: &App) -> Option<PendingUpdate> {
    cx.try_global::<PendingUpdateGlobal>()?.0.clone()
}

/// Opens the dialog for updating Pandora, which lists the games and servers that have to close
/// before the launcher can restart.
pub fn open_update_dialog(update: PendingUpdate, data: &DataEntities, window: &mut Window, cx: &mut App) {
    window.close_all_dialogs(cx);
    let state = cx.new(|cx| UpdateDialog::new(update, data.clone(), false, window, cx));
    window.open_dialog(cx, move |dialog, window, cx| {
        cx.update_entity(&state, |state, cx| state.render(dialog, window, cx))
    });
}

/// Updates straight away when nothing has to close first, and otherwise asks before closing it.
pub fn start_update(update: PendingUpdate, data: &DataEntities, window: &mut Window, cx: &mut App) {
    if running_instances(data, cx).is_empty() {
        match update {
            PendingUpdate::Available(update) => install(update, &data.backend_handle, window, cx),
            PendingUpdate::Restart(restart) => restart_launcher(&restart.relaunch, window, cx),
        }
    } else {
        open_update_dialog(update, data, window, cx);
    }
}

/// Asks whether to restart into a new version that's in place, and keeps offering it in the
/// title bar if the answer is later.
pub fn offer_restart(restart: PendingRestart, data: &DataEntities, window: &mut Window, cx: &mut App) {
    set_pending_update(PendingUpdate::Restart(restart.clone()), window, cx);
    open_update_dialog(PendingUpdate::Restart(restart), data, window, cx);
}

/// Called once an update is in place. Someone who chose to update has already said yes to the
/// restart, and to closing what's running for it.
pub fn update_installed(restart: PendingRestart, data: &DataEntities, window: &mut Window, cx: &mut App) {
    if !RESTART_WHEN_INSTALLED.swap(false, Ordering::Relaxed) {
        offer_restart(restart, data, window, cx);
        return;
    }

    set_pending_update(PendingUpdate::Restart(restart.clone()), window, cx);
    if running_instances(data, cx).is_empty() {
        restart_launcher(&restart.relaunch, window, cx);
        return;
    }

    window.close_all_dialogs(cx);
    let state = cx.new(|cx| UpdateDialog::new(PendingUpdate::Restart(restart), data.clone(), true, window, cx));
    window.open_dialog(cx, move |dialog, window, cx| {
        cx.update_entity(&state, |state, cx| state.render(dialog, window, cx))
    });
}

fn install(update: UpdatePrompt, backend_handle: &BackendHandle, window: &mut Window, cx: &mut App) {
    RESTART_WHEN_INSTALLED.store(true, Ordering::Relaxed);

    let modal_action = ModalAction::default();
    backend_handle.send(MessageToBackend::InstallUpdate {
        update,
        modal_action: modal_action.clone(),
    });
    window.close_all_dialogs(cx);
    crate::modals::generic::show_notification(window, cx, t::system::update::install_error().into(), modal_action);
}

/// The games and servers that keep the launcher from restarting.
fn running_instances(data: &DataEntities, cx: &App) -> Vec<InstanceEntry> {
    data.instances
        .read(cx)
        .entries
        .values()
        .map(|entry| entry.read(cx))
        .filter(|entry| entry.status != InstanceStatus::NotRunning)
        .cloned()
        .collect()
}

/// Asks `instance` to close. A server is told to stop, which saves its worlds, while a game is
/// asked to close and killed if it hasn't within a few seconds. `force` kills either.
fn close_instance(instance: &InstanceEntry, force: bool, backend_handle: &BackendHandle) {
    let id = instance.id;
    match instance.status {
        // Nothing to close until it has started, which the dialog waits for
        InstanceStatus::NotRunning | InstanceStatus::Launching => {},
        InstanceStatus::Running if instance.configuration.server.is_some() && !force => {
            backend_handle.send(MessageToBackend::StopServer { id });
        },
        InstanceStatus::Running => backend_handle.send(MessageToBackend::KillInstance { id }),
        InstanceStatus::Stopping if force => backend_handle.send(MessageToBackend::KillInstance { id }),
        InstanceStatus::Stopping => {},
    }
}

struct UpdateDialog {
    update: PendingUpdate,
    data: DataEntities,
    /// Closing what's running so the launcher can restart, which it does once all of it has.
    closing: bool,
    _subscriptions: [Subscription; 2],
}

impl UpdateDialog {
    fn new(
        update: PendingUpdate,
        data: DataEntities,
        closing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.subscribe_in::<_, InstanceModifiedEvent>(&data.instances, window, |this, _, event, window, cx| {
                // One that was still launching when the rest were closed only now has a process
                if this.closing && event.instance.status == InstanceStatus::Running {
                    close_instance(&event.instance, false, &this.data.backend_handle);
                }
                this.instances_changed(window, cx);
            }),
            cx.subscribe_in::<_, InstanceRemovedEvent>(&data.instances, window, |this, _, _, window, cx| {
                this.instances_changed(window, cx);
            }),
        ];

        let dialog = Self {
            update,
            data,
            closing,
            _subscriptions: subscriptions,
        };
        if closing {
            dialog.close_running(cx);
        }
        dialog
    }

    fn close_running(&self, cx: &App) {
        for instance in running_instances(&self.data, cx) {
            close_instance(&instance, false, &self.data.backend_handle);
        }
    }

    fn instances_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing
            && let PendingUpdate::Restart(restart) = &self.update
            && running_instances(&self.data, cx).is_empty()
        {
            self.closing = false;
            let relaunch = restart.relaunch.clone();
            // Not while this dialog is being updated, since restarting closes it
            window.defer(cx, move |window, cx| restart_launcher(&relaunch, window, cx));
        }
        cx.notify();
    }

    fn render(&mut self, dialog: Dialog, _window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        let running = running_instances(&self.data, cx);
        let theme = cx.theme();
        let (radius, row_background, muted, warning, success) =
            (theme.radius, theme.secondary, theme.muted_foreground, theme.warning, theme.success);

        let (title, summary) = match &self.update {
            PendingUpdate::Available(update) => {
                let size = if update.exe.size < 1000 * 10 {
                    t::system::update::size::bytes(update.exe.size)
                } else if update.exe.size < 1000 * 1000 * 10 {
                    t::system::update::size::kb(update.exe.size / 1000)
                } else if update.exe.size < 1000 * 1000 * 1000 * 10 {
                    t::system::update::size::mb(update.exe.size / 1000 / 1000)
                } else {
                    t::system::update::size::gb(update.exe.size / 1000 / 1000 / 1000)
                };
                let summary = v_flex()
                    .child(SharedString::from(t::system::update::current(&update.old_version)))
                    .child(SharedString::from(t::system::update::new(&update.new_version)))
                    .child(SharedString::from(size));
                (SharedString::from(t::system::update::title()), summary)
            },
            PendingUpdate::Restart(restart) => {
                let message: SharedString = match &restart.version {
                    Some(version) => t::system::update::restart::installed(version).into(),
                    None => t::system::update::restart::replaced().into(),
                };
                (t::system::update::restart::title().into(), v_flex().child(message))
            },
        };

        let changelog = Button::new("changelog")
            .small()
            .icon(PandoraIcon::Scroll)
            .label(t::system::update::changelog::view())
            .on_click(|_, window, cx| {
                window.close_all_dialogs(cx);
                crate::root::switch_page(PageType::Changelog, &[], window, cx);
            });

        let running_list = (!running.is_empty()).then(|| {
            let heading = if self.closing {
                t::system::update::running::waiting()
            } else {
                match self.update {
                    PendingUpdate::Available(_) => t::system::update::running::update(),
                    PendingUpdate::Restart(_) => t::system::update::running::restart(),
                }
            };

            let rows = running.iter().map(|instance| {
                let icon = match instance.icon.clone() {
                    Some(icon) => {
                        let transform = png_render_cache::ImageTransformation::Resize { width: 32, height: 32 };
                        png_render_cache::render_with_transform(icon, transform, cx)
                            .rounded(radius)
                            .size_6()
                            .min_w_6()
                            .into_any_element()
                    },
                    None => {
                        let icon = if instance.configuration.server.is_some() {
                            PandoraIcon::Server
                        } else {
                            PandoraIcon::Box
                        };
                        Icon::new(icon).size_5().min_w_6().into_any_element()
                    },
                };
                let kind = if instance.configuration.server.is_some() {
                    t::system::update::running::server()
                } else {
                    t::system::update::running::game()
                };
                let (status, color) = match instance.status {
                    InstanceStatus::Launching => (t::system::update::running::launching(), muted),
                    InstanceStatus::Stopping => (t::system::update::running::closing(), warning),
                    _ if self.closing => (t::system::update::running::closing(), warning),
                    _ => (t::system::update::running::running(), success),
                };

                h_flex()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded(radius)
                    .bg(row_background)
                    .child(icon)
                    .child(div().flex_1().truncate().child(instance.name.clone()))
                    .child(div().text_xs().text_color(muted).child(kind))
                    .child(div().w_20().text_right().text_sm().text_color(color).child(status))
            });

            v_flex()
                .gap_2()
                .child(div().text_sm().child(heading))
                .child(v_flex().gap_1().children(rows))
        });

        let buttons = if self.closing {
            h_flex()
                .w_full()
                .gap_2()
                .child(Button::new("force").flex_1().danger().label(t::system::update::force_close()).on_click(
                    cx.listener(|this, _, _, cx| {
                        for instance in running_instances(&this.data, cx) {
                            close_instance(&instance, true, &this.data.backend_handle);
                        }
                    }),
                ))
                .child(Button::new("cancel").flex_1().label(t::common::cancel()).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.closing = false;
                        window.close_all_dialogs(cx);
                    },
                )))
        } else {
            let label = match (&self.update, running.is_empty()) {
                (PendingUpdate::Available(_), true) => t::common::update(),
                (PendingUpdate::Available(_), false) => t::system::update::close_and_update(),
                (PendingUpdate::Restart(_), true) => t::system::update::restart::now(),
                (PendingUpdate::Restart(_), false) => t::system::update::close_and_restart(),
            };

            h_flex()
                .w_full()
                .gap_2()
                .child(Button::new("update").flex_1().success().label(label).on_click(cx.listener(
                    |this, _, window, cx| match this.update.clone() {
                        // Installing doesn't need anything closed, so it's left running until the
                        // update is in place and the restart is all that's left
                        PendingUpdate::Available(update) => install(update, &this.data.backend_handle, window, cx),
                        PendingUpdate::Restart(restart) => {
                            if running_instances(&this.data, cx).is_empty() {
                                restart_launcher(&restart.relaunch, window, cx);
                            } else {
                                this.closing = true;
                                this.close_running(cx);
                                cx.notify();
                            }
                        },
                    },
                )))
                .child(Button::new("later").flex_1().label(t::system::update::later()).on_click(|_, window, cx| {
                    window.close_all_dialogs(cx);
                }))
        };

        dialog.title(title).overlay_closable(false).w(px(460.0)).child(
            v_flex()
                .gap_4()
                .child(summary)
                .child(h_flex().child(changelog))
                .children(running_list)
                .child(buttons),
        )
    }
}

/// Starts the launcher at `relaunch` and quits this one, which the new one waits for.
fn restart_launcher(relaunch: &Path, window: &mut Window, cx: &mut App) {
    if let Err(err) = bridge::launcher::spawn_restart(relaunch) {
        log::error!("Unable to start {:?} to restart into: {}", relaunch, err);
        let message = SharedString::from(t::system::update::restart::error(err));
        window.push_notification((NotificationType::Error, message), cx);
        return;
    }

    window.close_all_dialogs(cx);
    cx.defer(crate::quit);
}
