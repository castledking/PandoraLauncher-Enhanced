use bridge::{
    handle::BackendHandle,
    instance::InstanceID,
    message::MessageToBackend,
    modal_action::ModalAction,
    server_sync::{ServerSyncPlan, ServerSyncSide},
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Sizable, StyledExt, WindowExt,
    alert::Alert,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    dialog::Dialog,
    h_flex,
    scroll::ScrollableElement,
    spinner::Spinner,
    v_flex,
};

use crate::icon::PandoraIcon;

enum PlanState {
    Loading,
    Failed,
    Loaded(ServerSyncPlan),
}

struct ServerSyncModalState {
    backend_handle: BackendHandle,
    plan: PlanState,
    _load_task: Task<()>,
}

impl ServerSyncModalState {
    fn new(
        server_id: InstanceID,
        client_id: InstanceID,
        backend_handle: BackendHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let (send, recv) = tokio::sync::oneshot::channel();
        backend_handle.send(MessageToBackend::PlanServerSync {
            server_id,
            client_id,
            channel: send,
        });

        let load_task = cx.spawn(async move |this, cx| {
            let plan = recv.await.ok().flatten();
            _ = this.update(cx, |this, cx| {
                this.plan = match plan {
                    Some(plan) => PlanState::Loaded(plan),
                    None => PlanState::Failed,
                };
                cx.notify();
            });
        });

        Self {
            backend_handle,
            plan: PlanState::Loading,
            _load_task: load_task,
        }
    }

    fn set_all(&mut self, selected: bool, cx: &mut Context<Self>) {
        if let PlanState::Loaded(plan) = &mut self.plan {
            let mut mods = plan.mods.to_vec();
            for entry in &mut mods {
                entry.selected = selected;
            }
            plan.mods = mods.into();
            cx.notify();
        }
    }

    fn toggle(&mut self, index: usize, selected: bool, cx: &mut Context<Self>) {
        if let PlanState::Loaded(plan) = &mut self.plan {
            let mut mods = plan.mods.to_vec();
            if let Some(entry) = mods.get_mut(index) {
                entry.selected = selected;
            }
            plan.mods = mods.into();
            cx.notify();
        }
    }

    fn render(&mut self, dialog: Dialog, _window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        let dialog = dialog.overlay_closable(false).w(px(640.0));

        let plan = match &self.plan {
            PlanState::Loading => {
                return dialog.title(t::server::sync::title_generic()).child(
                    h_flex()
                        .w_full()
                        .h_32()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .child(Spinner::new())
                        .child(t::server::sync::scanning()),
                );
            },
            PlanState::Failed => {
                return dialog
                    .title(t::server::sync::title_generic())
                    .child(Alert::error("sync-failed", t::server::sync::failed()))
                    .footer(
                        Button::new("close")
                            .label(t::common::ok())
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    );
            },
            PlanState::Loaded(plan) => plan.clone(),
        };

        let theme = cx.theme();

        let mut to_copy = 0;
        let mut to_remove = 0;
        let mut skipped = 0;
        for entry in plan.mods.iter() {
            match (entry.selected, entry.only_on_server, entry.already_on_server) {
                (false, _, _) => skipped += 1,
                (true, true, _) => to_remove += 1,
                (true, false, false) => to_copy += 1,
                (true, false, true) => {},
            }
        }

        let warnings = v_flex()
            .gap_2()
            .when_some(plan.version_mismatch.clone(), |this, mismatch| {
                this.child(
                    Alert::warning("version-mismatch", t::server::sync::version_mismatch(&mismatch))
                        .icon(PandoraIcon::TriangleAlert),
                )
            })
            .when_some(plan.loader_mismatch.clone(), |this, mismatch| {
                this.child(
                    Alert::warning("loader-mismatch", t::server::sync::loader_mismatch(&mismatch))
                        .icon(PandoraIcon::TriangleAlert),
                )
            });

        let summary = h_flex()
            .gap_2()
            .text_sm()
            .child(summary_chip(t::server::sync::to_copy(to_copy), theme.success, cx))
            .child(summary_chip(t::server::sync::to_remove(to_remove), theme.danger, cx))
            .child(summary_chip(t::server::sync::skipped(skipped), theme.muted_foreground, cx))
            .child(div().flex_1())
            .child(
                Button::new("select-all")
                    .ghost()
                    .xsmall()
                    .label(t::server::sync::select_all())
                    .on_click(cx.listener(|this, _, _, cx| this.set_all(true, cx))),
            )
            .child(
                Button::new("select-none")
                    .ghost()
                    .xsmall()
                    .label(t::server::sync::select_none())
                    .on_click(cx.listener(|this, _, _, cx| this.set_all(false, cx))),
            );

        let rows = plan.mods.iter().enumerate().map(|(index, entry)| {
            let (state_label, state_color) = if entry.only_on_server {
                (t::server::sync::state_remove(), theme.danger)
            } else if entry.already_on_server {
                (t::server::sync::state_present(), theme.muted_foreground)
            } else {
                (t::server::sync::state_new(), theme.success)
            };

            let side_color = match entry.side {
                ServerSyncSide::ClientOnly => theme.warning,
                ServerSyncSide::ServerOnly | ServerSyncSide::Both => theme.info,
                ServerSyncSide::Unknown => theme.muted_foreground,
            };

            h_flex()
                .id(("sync-row", index))
                .w_full()
                .gap_3()
                .px_3()
                .py_1p5()
                .rounded(theme.radius)
                .hover(|this| this.bg(theme.secondary.opacity(0.5)))
                .when(!entry.selected, |this| this.opacity(0.55))
                .child(Checkbox::new(("sync-check", index)).checked(entry.selected).on_click(cx.listener(
                    move |this, value: &bool, _, cx| {
                        this.toggle(index, *value, cx);
                    },
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(SharedString::from(entry.filename.to_string())),
                )
                .when(!entry.only_on_server, |this| this.child(badge(entry.side.label(), side_color)))
                .child(badge(state_label, state_color))
        });

        let list = v_flex()
            .max_h(px(360.0))
            .min_h_24()
            .p_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .overflow_y_scrollbar()
            .children(rows)
            .when(plan.mods.is_empty(), |this| {
                this.child(div().p_4().text_sm().text_color(theme.muted_foreground).child(t::server::sync::nothing()))
            });

        let backend_handle = self.backend_handle.clone();
        let apply_plan = plan.clone();

        dialog
            .title(t::server::sync::title(&plan.client_name, &plan.server_name))
            .child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().text_color(theme.muted_foreground).child(t::server::sync::explanation()))
                    .child(warnings)
                    .child(summary)
                    .child(list),
            )
            .footer(
                h_flex()
                    .gap_2()
                    .w_full()
                    .child(
                        Button::new("cancel")
                            .flex_1()
                            .label(t::common::cancel())
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("apply")
                            .flex_1()
                            .success()
                            .icon(PandoraIcon::RefreshCcw)
                            .label(t::server::sync::apply())
                            .disabled(to_copy == 0 && to_remove == 0)
                            .on_click(move |_, window, cx| {
                                let modal_action = ModalAction::default();
                                backend_handle.send(MessageToBackend::ApplyServerSync {
                                    plan: apply_plan.clone(),
                                    modal_action: modal_action.clone(),
                                });
                                window.close_dialog(cx);
                                crate::modals::generic::show_notification(
                                    window,
                                    cx,
                                    t::server::sync::failed().into(),
                                    modal_action,
                                );
                            }),
                    ),
            )
    }
}

fn badge(label: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .px_1p5()
        .py_0p5()
        .rounded_sm()
        .text_xs()
        .font_medium()
        .text_color(color)
        .bg(color.opacity(0.12))
        .child(label.into())
}

fn summary_chip(label: impl Into<SharedString>, color: Hsla, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .child(div().size_2().rounded_full().bg(color))
        .child(label.into())
}

pub fn open_server_sync(
    server_id: InstanceID,
    client_id: InstanceID,
    backend_handle: BackendHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let state = cx.new(|cx| ServerSyncModalState::new(server_id, client_id, backend_handle, cx));
    window.open_dialog(cx, move |dialog, window, cx| {
        cx.update_entity(&state, |state, cx| state.render(dialog, window, cx))
    });
}
