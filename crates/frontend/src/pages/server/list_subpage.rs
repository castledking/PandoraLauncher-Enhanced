use std::sync::Arc;

use bridge::{handle::BackendHandle, instance::InstanceID, message::MessageToBackend};
use gpui::{prelude::*, *};
use gpui_component::Theme;
use gpui_component::{
    ActiveTheme, Disableable, Icon, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    scroll::ScrollableElement,
    spinner::Spinner,
    v_flex,
};
use schema::server_list::{ServerListEntry, ServerListFile};

use crate::{entity::instance::InstanceEntry, icon::PandoraIcon};

/// The names typed so far in the add dialog. Held as an entity so the buttons can change it
/// and have the dialog redraw.
#[derive(Default)]
struct NameDraft {
    names: Vec<Arc<str>>,
}

enum LoadState {
    Loading,
    Loaded,
}

pub struct ServerListSubpage {
    instance_id: InstanceID,
    backend_handle: BackendHandle,
    file: ServerListFile,
    entries: Arc<[ServerListEntry]>,
    state: LoadState,
    search: Entity<InputState>,
    /// Whether the file is shown as it sits on disk rather than as a list of entries.
    raw: bool,
    raw_input: Entity<TextareaState>,
    raw_dirty: bool,
    _load_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl ServerListSubpage {
    pub fn new(
        instance: &Entity<InstanceEntry>,
        backend_handle: BackendHandle,
        file: ServerListFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t::server::lists::search()));
        let raw_input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 12));

        let subscriptions = vec![
            cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&raw_input, |this, _, _: &InputEvent, cx| {
                this.raw_dirty = true;
                cx.notify();
            }),
        ];

        let mut this = Self {
            instance_id: instance.read(cx).id,
            backend_handle,
            file,
            entries: Arc::new([]),
            state: LoadState::Loading,
            search,
            raw: false,
            raw_input,
            raw_dirty: false,
            _load_task: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.reload(window, cx);
        this
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state = LoadState::Loading;

        let (send_list, recv_list) = tokio::sync::oneshot::channel();
        let (send_raw, recv_raw) = tokio::sync::oneshot::channel();
        self.backend_handle.send(MessageToBackend::GetServerList {
            id: self.instance_id,
            file: self.file,
            channel: send_list,
        });
        self.backend_handle.send(MessageToBackend::GetServerListRaw {
            id: self.instance_id,
            file: self.file,
            channel: send_raw,
        });

        self._load_task = cx.spawn_in(window, async move |this, cx| {
            let entries = recv_list.await.unwrap_or_default();
            let raw = recv_raw.await.unwrap_or_default();

            _ = this.update_in(cx, |this, window, cx| {
                this.entries = entries;
                this.state = LoadState::Loaded;

                // Repopulated only when it isn't holding unsaved edits, so coming back from
                // the raw file doesn't throw away something being typed
                if !this.raw_dirty {
                    this.raw_input.update(cx, |input, cx| {
                        input.set_value(raw.to_string(), window, cx);
                    });
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    fn save_raw(&mut self, cx: &mut Context<Self>) {
        let contents: Arc<str> = self.raw_input.read(cx).value().into();
        self.backend_handle.send(MessageToBackend::SetServerListRaw {
            id: self.instance_id,
            file: self.file,
            contents,
        });
        self.raw_dirty = false;
        cx.notify();
    }

    /// Asks for the names to be added. The backend resolves them against Mojang and reports
    /// afterwards which ones didn't match an account.
    fn open_add_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.file;
        let instance_id = self.instance_id;
        let backend_handle = self.backend_handle.clone();

        window.open_dialog(cx, move |dialog, window, cx| {
            let draft = cx.new(|_| NameDraft::default());
            let name_input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t::server::lists::dialog::name_placeholder())
                    .clean_on_escape()
            });
            let reason_input =
                cx.new(|cx| InputState::new(window, cx).placeholder(t::server::lists::dialog::reason_placeholder()));

            // Put the cursor in the name box, since the whole dialog is about typing one
            name_input.update(cx, |input, cx| input.focus(window, cx));

            // The dialog lays out its own content, so the spacing comes from a column inside
            // it rather than from styles set on the dialog, which it draws over.
            dialog.title(dialog_title(file)).overlay_closable(true).child(
                v_flex()
                    .gap_3()
                    .child(h_flex().gap_2().child(Input::new(&name_input).flex_1()).child(
                        Button::new("add-name").icon(PandoraIcon::Plus).on_click({
                            let draft = draft.clone();
                            let name_input = name_input.clone();
                            move |_, window, cx| {
                                let value: Arc<str> = name_input.read(cx).value().trim().into();
                                if value.is_empty() {
                                    return;
                                }

                                let added = draft.update(cx, |draft, cx| {
                                    if draft.names.contains(&value) {
                                        return false;
                                    }
                                    draft.names.push(value);
                                    cx.notify();
                                    true
                                });

                                if added {
                                    name_input.update(cx, |input, cx| {
                                        input.set_value(String::new(), window, cx);
                                    });
                                }
                            }
                        }),
                    ))
                    .child(v_flex().gap_1().max_h(px(160.0)).overflow_y_scrollbar().children(
                        draft.read(cx).names.iter().enumerate().map(|(index, name)| {
                            h_flex()
                                .gap_2()
                                .text_sm()
                                .child(
                                    div()
                                        .w_5()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("{}.", index + 1)),
                                )
                                .child(div().flex_1().truncate().child(name.to_string()))
                                .child(
                                    Button::new(("remove-name", index))
                                        .ghost()
                                        .small()
                                        .compact()
                                        .icon(PandoraIcon::Close)
                                        .on_click({
                                            let draft = draft.clone();
                                            let name = name.clone();
                                            move |_, _, cx| {
                                                draft.update(cx, |draft, cx| {
                                                    draft.names.retain(|existing| existing != &name);
                                                    cx.notify();
                                                });
                                            }
                                        }),
                                )
                        }),
                    ))
                    .when(file != ServerListFile::BannedIps, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().w(px(112.0)).text_sm().child(t::server::lists::dialog::reason()))
                                .child(Input::new(&reason_input).flex_1()),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .mt_2()
                            .justify_end()
                            .child(
                                Button::new("cancel")
                                    .label(t::common::cancel())
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(confirm_button(file).disabled(draft.read(cx).names.is_empty()).on_click({
                                let draft = draft.clone();
                                let reason_input = reason_input.clone();
                                let backend_handle = backend_handle.clone();
                                move |_, window, cx| {
                                    let names = Arc::from(draft.read(cx).names.as_slice());
                                    let reason: Arc<str> = reason_input.read(cx).value().into();
                                    let reason = reason.trim();
                                    let reason = (!reason.is_empty()).then(|| Arc::from(reason));

                                    // The outcome isn't waited on here. The backend
                                    // warns about names it couldn't resolve, which
                                    // is the only part worth interrupting for.
                                    let (send, _recv) = tokio::sync::oneshot::channel();
                                    backend_handle.send(MessageToBackend::AddServerListEntries {
                                        id: instance_id,
                                        file,
                                        names,
                                        reason,
                                        channel: send,
                                    });
                                    window.close_dialog(cx);
                                }
                            })),
                    ),
            )
        });
    }
}

impl Render for ServerListSubpage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        let toolbar = h_flex()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .when(!self.raw, |this| {
                this.child(Input::new(&self.search).w(px(220.0)).small()).child(div().flex_1())
            })
            .when(self.raw, |this| this.child(div().flex_1()))
            // Left out entirely for the user cache. It is a record of who the server has
            // already seen, written as players join, so there is nothing to add to it.
            .when(self.file.is_editable(), |this| {
                this.child(
                    add_button(self.file).on_click(cx.listener(|this, _, window, cx| this.open_add_dialog(window, cx))),
                )
            })
            .child(
                Button::new("toggle-raw")
                    .icon(PandoraIcon::File)
                    .label(if self.raw {
                        t::server::lists::formatted()
                    } else {
                        t::server::lists::raw()
                    })
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.raw = !this.raw;
                        cx.notify();
                    })),
            );

        if matches!(self.state, LoadState::Loading) {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element();
        }

        if self.raw {
            return v_flex()
                .size_full()
                .child(toolbar)
                .child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .gap_2()
                        .p_4()
                        .child(Textarea::new(&self.raw_input).flex_1().min_h_0())
                        .child(
                            h_flex().justify_end().child(
                                Button::new("save-raw")
                                    .success()
                                    .label(t::server::lists::save_raw())
                                    .disabled(!self.raw_dirty)
                                    .on_click(cx.listener(|this, _, _, cx| this.save_raw(cx))),
                            ),
                        ),
                )
                .into_any_element();
        }

        let query = self.search.read(cx).value().to_lowercase();
        let shown: Vec<&ServerListEntry> = self
            .entries
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.name().to_lowercase().contains(query.as_str())
                    || entry.reason().is_some_and(|reason| reason.to_lowercase().contains(query.as_str()))
            })
            .collect();

        let empty = shown.is_empty();

        // Built up front rather than lazily. The rows hold listeners, and a listener has
        // to be able to reach back into the page after the list it was built from is gone.
        let mut rows = Vec::with_capacity(shown.len());
        for (index, entry) in shown.into_iter().enumerate() {
            let instance_id = self.instance_id;
            let file = self.file;
            let backend_handle = self.backend_handle.clone();
            let (name, uuid) = (entry.name(), entry.uuid());

            let remove = move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                backend_handle.send(MessageToBackend::RemoveServerListEntry {
                    id: instance_id,
                    file,
                    name: name.clone(),
                    uuid,
                });
                this.reload(window, cx);
            };

            rows.push(render_entry(&theme, index, entry.name(), entry.reason(), file.is_editable(), cx, remove));
        }

        let body = if empty {
            let searching = !query.is_empty();
            let editable = self.file.is_editable();

            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    v_flex()
                        .items_center()
                        .gap_3()
                        .max_w(px(360.0))
                        .child(
                            div()
                                .size_12()
                                .rounded_full()
                                .bg(theme.secondary)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(file_icon(self.file)).size_6().text_color(theme.muted_foreground)),
                        )
                        .child(div().font_semibold().child(if searching {
                            t::server::lists::no_match().into()
                        } else {
                            empty_title(self.file)
                        }))
                        .when(!searching, |this| {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_center()
                                    .text_color(theme.muted_foreground)
                                    .child(empty_body(self.file)),
                            )
                        })
                        // Only offered where there's something to add to. The user cache is
                        // written by the server, so an Add here would be a lie.
                        .when(!searching && editable, |this| {
                            this.child(
                                add_button(self.file)
                                    .on_click(cx.listener(|this, _, window, cx| this.open_add_dialog(window, cx))),
                            )
                        }),
                )
                .into_any_element()
        } else {
            v_flex().gap_1().p_2().children(rows).into_any_element()
        };

        v_flex()
            .size_full()
            .child(toolbar)
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(body))
            .into_any_element()
    }
}

/// One entry in the list. The remove listener is handed in already bound to this row's
/// name, since each row removes a different thing.
fn render_entry(
    theme: &Theme,
    index: usize,
    name: Arc<str>,
    reason: Option<Arc<str>>,
    editable: bool,
    cx: &mut Context<ServerListSubpage>,
    remove: impl Fn(&mut ServerListSubpage, &mut Window, &mut Context<ServerListSubpage>) + 'static,
) -> AnyElement {
    h_flex()
        .gap_3()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .items_center()
        .child(v_flex().flex_1().min_w_0().child(div().truncate().child(name.to_string())).when(
            reason.is_some(),
            |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(reason.map(|reason| reason.to_string()).unwrap_or_default()),
                )
            },
        ))
        .when(editable, |this| {
            this.child(
                Button::new(("remove", index))
                    .ghost()
                    .small()
                    .compact()
                    .icon(PandoraIcon::Trash2)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        remove(this, window, cx);
                    })),
            )
        })
        .into_any_element()
}

fn confirm_button(file: ServerListFile) -> Button {
    let button = Button::new("confirm").label(dialog_confirm(file));

    match file {
        ServerListFile::BannedPlayers | ServerListFile::BannedIps => button.danger(),
        ServerListFile::Whitelist | ServerListFile::Operators => button.success(),
        ServerListFile::UserCache => button.primary(),
    }
}

/// An Add button for this file, coloured by what adding a name to it does: refusing
/// someone, letting them in, or handing them the server.
fn add_button(file: ServerListFile) -> Button {
    let button = Button::new("add").icon(PandoraIcon::Plus).label(add_label(file));

    match file {
        ServerListFile::BannedPlayers | ServerListFile::BannedIps => button.danger(),
        ServerListFile::Whitelist | ServerListFile::Operators => button.success(),
        ServerListFile::UserCache => button.primary(),
    }
}

/// The same icons the tabs use, so a list and the tab that opened it read as one thing.
fn file_icon(file: ServerListFile) -> PandoraIcon {
    match file {
        ServerListFile::BannedPlayers => PandoraIcon::CircleX,
        ServerListFile::BannedIps => PandoraIcon::Globe,
        ServerListFile::Whitelist => PandoraIcon::CircleCheck,
        ServerListFile::Operators => PandoraIcon::Award,
        ServerListFile::UserCache => PandoraIcon::Users,
    }
}

fn add_label(file: ServerListFile) -> SharedString {
    match file {
        ServerListFile::BannedPlayers => t::server::lists::add_ban().into(),
        ServerListFile::BannedIps => t::server::lists::add_ip().into(),
        ServerListFile::Whitelist => t::server::lists::add_whitelist().into(),
        ServerListFile::Operators => t::server::lists::add_operator().into(),
        ServerListFile::UserCache => SharedString::default(),
    }
}

fn dialog_title(file: ServerListFile) -> SharedString {
    match file {
        ServerListFile::BannedPlayers => t::server::lists::dialog::title_ban().into(),
        ServerListFile::BannedIps => t::server::lists::dialog::title_ip().into(),
        ServerListFile::Whitelist => t::server::lists::dialog::title_whitelist().into(),
        ServerListFile::Operators => t::server::lists::dialog::title_operator().into(),
        ServerListFile::UserCache => SharedString::default(),
    }
}

fn dialog_confirm(file: ServerListFile) -> SharedString {
    match file {
        ServerListFile::BannedPlayers => t::server::lists::dialog::confirm_ban().into(),
        ServerListFile::BannedIps => t::server::lists::dialog::confirm_ip().into(),
        ServerListFile::Whitelist => t::server::lists::dialog::confirm_whitelist().into(),
        ServerListFile::Operators => t::server::lists::dialog::confirm_operator().into(),
        ServerListFile::UserCache => SharedString::default(),
    }
}

fn empty_title(file: ServerListFile) -> SharedString {
    match file {
        ServerListFile::BannedPlayers => t::server::lists::empty_bans().into(),
        ServerListFile::BannedIps => t::server::lists::empty_ips().into(),
        ServerListFile::Whitelist => t::server::lists::empty_whitelist().into(),
        ServerListFile::Operators => t::server::lists::empty_operators().into(),
        ServerListFile::UserCache => t::server::lists::empty_user_cache().into(),
    }
}

fn empty_body(file: ServerListFile) -> SharedString {
    match file {
        ServerListFile::BannedPlayers => t::server::lists::empty_bans_body().into(),
        ServerListFile::BannedIps => t::server::lists::empty_ips_body().into(),
        ServerListFile::Whitelist => t::server::lists::empty_whitelist_body().into(),
        ServerListFile::Operators => t::server::lists::empty_operators_body().into(),
        ServerListFile::UserCache => t::server::lists::empty_user_cache_body().into(),
    }
}
