use std::sync::Arc;

use crate::{
    entity::instance::InstanceEntry,
    icon::PandoraIcon,
    png_render_cache::{self, ImageTransformation},
    skin_thumbnail_cache::SkinThumbnailCache,
};
use bridge::{
    handle::BackendHandle,
    instance::InstanceID,
    message::{AccountSkinResult, MessageToBackend},
};
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
use rustc_hash::{FxHashMap, FxHashSet};
use schema::{
    minecraft_profile::SkinVariant,
    server_list::{ServerListEntry, ServerListFile},
    unique_bytes::UniqueBytes,
};
use uuid::Uuid;

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

/// How an entry is drawn beside its name. All three come off the same skin.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerListDisplay {
    /// The head, flat, cropped out of the skin the way the account list shows one.
    #[default]
    Head2d,
    /// The whole skin texture, flat, the way the skins page shows one.
    Thumbnail,
}

impl ServerListDisplay {
    fn next(self) -> Self {
        match self {
            ServerListDisplay::Head2d => ServerListDisplay::Thumbnail,
            ServerListDisplay::Thumbnail => ServerListDisplay::Head2d,
        }
    }
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
    display: ServerListDisplay,
    thumbnails: Entity<SkinThumbnailCache>,
    /// Skins already fetched, by the UUID on the entry. A name with no UUID, or a UUID
    /// Mojang has nothing for, simply never turns up here and is shown by name alone.
    skins: FxHashMap<Uuid, UniqueBytes>,
    requested: FxHashSet<Uuid>,
    _load_task: Task<()>,
    /// Waiting on changes to reach the file, each reloading the list once it has.
    _change_tasks: Vec<Task<()>>,
    _skin_tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AddNamesDialogEvent> for ServerListSubpage {}

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

        let thumbnails = SkinThumbnailCache::new(cx);

        let subscriptions = vec![
            cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&raw_input, |this, _, _: &InputEvent, cx| {
                this.raw_dirty = true;
                cx.notify();
            }),
            // The cache isn't an event emitter, but it does redraw itself when a render
            // lands, and watching for that is what puts the thumbnail on screen
            cx.observe(&thumbnails, |_, _, cx| cx.notify()),
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
            display: ServerListDisplay::default(),
            thumbnails,
            skins: FxHashMap::default(),
            requested: FxHashSet::default(),
            _load_task: Task::ready(()),
            _change_tasks: Vec::new(),
            _skin_tasks: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.reload(window, cx);
        this
    }

    /// Reloads the list once `done` says a change has been saved.
    ///
    /// Not straight away: a running server is asked over its console and saves the file in its
    /// own time, so reading it at once still found it as it was, and a player just added only
    /// showed up after leaving the page and coming back.
    fn reload_when<T: 'static>(
        &mut self,
        done: tokio::sync::oneshot::Receiver<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._change_tasks.retain(|task| !task.is_ready());
        self._change_tasks.push(cx.spawn_in(window, async move |this, cx| {
            _ = done.await;
            _ = this.update_in(cx, |this, window, cx| this.reload(window, cx));
        }));
    }

    /// Adds `names` to the list, which the backend resolves against Mojang and reports
    /// afterwards which ones didn't match an account.
    fn add_names(
        &mut self,
        names: Arc<[Arc<str>]>,
        reason: Option<Arc<str>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The backend warns about names it couldn't resolve itself, which is the only part of
        // the outcome worth interrupting for
        let (send, recv) = tokio::sync::oneshot::channel();
        self.backend_handle.send(MessageToBackend::AddServerListEntries {
            id: self.instance_id,
            file: self.file,
            names,
            reason,
            channel: send,
        });
        self.reload_when(recv, window, cx);
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
                this.request_skins(cx);

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

    /// Asks for the skin behind each name that has a UUID behind it.
    ///
    /// A banned address has no player to it, and a name Mojang has no account for has no
    /// UUID, so those are simply shown by name. Anything already asked for or already
    /// known is left alone, since a list can be reloaded often.
    fn request_skins(&mut self, cx: &mut Context<Self>) {
        if !self.file.describes_players() {
            return;
        }

        for uuid in self.entries.iter().filter_map(|entry| entry.uuid()) {
            if self.requested.contains(&uuid) {
                continue;
            }
            self.requested.insert(uuid);

            let (send, recv) = tokio::sync::oneshot::channel();
            self.backend_handle.send(MessageToBackend::GetAccountSkin {
                account: uuid,
                result: send,
            });

            self._skin_tasks.push(cx.spawn(async move |this, cx| {
                let skin = recv.await.ok().and_then(|result| match result {
                    AccountSkinResult::Success { skin, .. } => skin,
                    AccountSkinResult::NeedsLogin | AccountSkinResult::UnableToLoadSkin => None,
                });

                _ = this.update(cx, |this, cx| {
                    if let Some(skin) = skin {
                        this.skins.insert(uuid, skin);
                    }
                    cx.notify();
                });
            }));
        }
    }

    /// The picture to draw beside a name, in whichever of the three ways is set.
    ///
    /// A name with no UUID behind it, or one Mojang has nothing for, gets nothing here and
    /// is shown by name alone rather than as a placeholder that means nothing.
    fn picture_for(&mut self, uuid: Uuid, cx: &mut Context<Self>) -> Option<AnyElement> {
        let skin = self.skins.get(&uuid)?.clone();

        match self.display {
            ServerListDisplay::Head2d => {
                // The face out of the top left of the skin, which is where Minecraft keeps
                // it. The hat layer is a second piece drawn on top and is left off here;
                // a flat crop can't show both without compositing them first.
                const HEAD: ImageTransformation = ImageTransformation::CropAndScale {
                    min_x: 8,
                    min_y: 8,
                    width: 8,
                    height: 8,
                    scale: 6,
                };
                Some(
                    png_render_cache::render_with_transform(skin, HEAD, cx)
                        .w(px(28.0))
                        .h(px(28.0))
                        .into_any_element(),
                )
            },
            ServerListDisplay::Thumbnail => {
                let variant = SkinVariant::Classic;
                let thumbnails = self.thumbnails.update(cx, |cache, cx| cache.get_or_queue(&skin, variant, cx));

                thumbnails.map(|(front, _)| {
                    gpui::img(front)
                        .w(px(28.0))
                        .h(px(28.0))
                        .object_fit(gpui::ObjectFit::Contain)
                        .into_any_element()
                })
            },
        }
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

    /// Asks for the names to be added.
    fn open_add_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.file;
        let body = cx.new(|cx| AddNamesDialog::new(file, window, cx));

        // The dialog sits in its own window on top of this page, so nothing here is told
        // when it closes. Without this the list kept showing what it was showing until
        // something else happened to redraw the page, which meant a ban you had just made
        // only appeared after going to another tab and back.
        self._subscriptions
            .push(cx.subscribe_in(&body, window, |this, _, event: &AddNamesDialogEvent, window, cx| {
                this.add_names(event.names.clone(), event.reason.clone(), window, cx);
            }));

        // Only the frame and the title live here. The contents are a view of their own, so
        // that typing redraws them.
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title(dialog_title(file)).overlay_closable(true).child(body.clone())
        });
    }
}

/// The names the dialog was confirmed with, for the page behind it to add.
struct AddNamesDialogEvent {
    names: Arc<[Arc<str>]>,
    reason: Option<Arc<str>>,
}

/// The body of the add dialog: a name box, the batch being built up, and the buttons.
///
/// Its own view rather than part of the dialog's own layout, so that typing redraws it.
/// The dialog is drawn by the root, and the root is not told when an input inside a dialog
/// changes, so a button worked out from what had been typed sat greyed out until
/// something else happened to redraw it.
struct AddNamesDialog {
    file: ServerListFile,
    draft: Entity<NameDraft>,
    name_input: Entity<InputState>,
    reason_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl AddNamesDialog {
    fn new(file: ServerListFile, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = cx.new(|_| NameDraft::default());
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t::server::lists::dialog::name_placeholder())
                .clean_on_escape()
        });
        let reason_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::server::lists::dialog::reason_placeholder()));

        // Redraw on anything that changes what the buttons should be doing
        let subscriptions = vec![
            cx.subscribe(&name_input, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&reason_input, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.observe(&draft, |_, _, cx| cx.notify()),
        ];

        Self {
            file,
            draft,
            name_input,
            reason_input,
            _subscriptions: subscriptions,
        }
    }
}

impl EventEmitter<AddNamesDialogEvent> for AddNamesDialog {}

impl Render for AddNamesDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let file = self.file;
        let typed = self.name_input.read(cx).value();
        let pending = pending_names(&self.draft.read(cx), &typed);

        let draft = self.draft.clone();
        let name_input = self.name_input.clone();

        v_flex()
            .gap_3()
            .child(h_flex().gap_2().child(Input::new(&self.name_input).flex_1()).child(
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
                self.draft.read(cx).names.iter().enumerate().map(|(index, name)| {
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .child(div().w_5().text_color(theme.muted_foreground).child(format!("{}.", index + 1)))
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
            // Only a ban has anywhere to put a reason. Whitelist and operator entries have
            // no such field, so the box was asking for something that got thrown away.
            .when(file == ServerListFile::BannedPlayers, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().w(px(112.0)).text_sm().child(t::server::lists::dialog::reason()))
                        .child(Input::new(&self.reason_input).flex_1()),
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
                    .child(confirm_button(file).disabled(pending.is_empty()).on_click({
                        let draft = draft.clone();
                        let name_input = name_input.clone();
                        let reason_input = self.reason_input.clone();
                        // A handle to this view, so the event can be sent from here rather
                        // than from whoever opened the dialog
                        let dialog = cx.entity();
                        move |_, window, cx| {
                            // Anything still sitting in the box counts as one more
                            // name, so banning a single player is typing a name and
                            // pressing the button, rather than two steps.
                            let names =
                                Arc::from(pending_names(&draft.read(cx), &name_input.read(cx).value()).as_slice());

                            let reason: Arc<str> = reason_input.read(cx).value().into();
                            let reason = reason.trim();
                            let reason = (!reason.is_empty()).then(|| Arc::from(reason));

                            // Handed to the page, which adds them and reloads the list once
                            // they're in. The dialog closes itself straight after, and the
                            // page has no other way of hearing about it.
                            cx.update_entity(&dialog, |_, cx| cx.emit(AddNamesDialogEvent { names, reason }));
                            window.close_dialog(cx);
                        }
                    })),
            )
    }
}

impl Render for ServerListSubpage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        // Nothing to search, filter or edit until something is in the file, and the empty
        // state below carries the only button that does anything. Raw view keeps its
        // toolbar in case the file is emptied out from under it.
        let show_toolbar = !self.entries.is_empty() || self.raw;

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
            // Only the lists of players have anyone to show a skin of. A banned address
            // is just an address, and the user cache is the server's own record.
            .when(self.file.describes_players(), |this| {
                this.child(
                    Button::new("toggle-display")
                        .small()
                        .compact()
                        .icon(match self.display {
                            ServerListDisplay::Head2d => PandoraIcon::Image,
                            ServerListDisplay::Thumbnail => PandoraIcon::PackageOpen,
                        })
                        .tooltip(match self.display {
                            ServerListDisplay::Head2d => t::server::lists::display::head_2d(),
                            ServerListDisplay::Thumbnail => t::server::lists::display::thumbnail(),
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.display = this.display.next();
                            cx.notify();
                        })),
                )
            })
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
                .when(show_toolbar, |this| this.child(toolbar))
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
        // Held by value rather than borrowed, so that working out each row's picture can
        // borrow the page itself. The rows take what they need out of it anyway.
        let shown: Vec<ServerListEntry> = self
            .entries
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.name().to_lowercase().contains(query.as_str())
                    || entry.reason().is_some_and(|reason| reason.to_lowercase().contains(query.as_str()))
            })
            .cloned()
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
                let (send, recv) = tokio::sync::oneshot::channel();
                backend_handle.send(MessageToBackend::RemoveServerListEntry {
                    id: instance_id,
                    file,
                    name: name.clone(),
                    uuid,
                    channel: send,
                });
                this.reload_when(recv, window, cx);
            };

            let picture = entry.uuid().and_then(|uuid| self.picture_for(uuid, cx));
            rows.push(render_entry(
                &theme,
                index,
                entry.name(),
                entry.reason(),
                file.is_editable(),
                picture,
                cx,
                remove,
            ));
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
            .when(show_toolbar, |this| this.child(toolbar))
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
    picture: Option<AnyElement>,
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
        .when_some(picture, |this, picture| this.child(picture))
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

/// The names a confirm would submit: the ones already added to the batch, plus whatever is
/// still sitting in the box and hasn't been added yet.
fn pending_names(draft: &NameDraft, typed: &str) -> Vec<Arc<str>> {
    let typed: Arc<str> = typed.trim().into();

    let mut names = draft.names.clone();
    if !typed.is_empty() && !names.contains(&typed) {
        names.push(typed);
    }

    names
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
