use std::{
    ffi::OsString,
    sync::{Arc, atomic::{AtomicUsize, Ordering}},
};

use bridge::{
    handle::BackendHandle,
    instance::{InstanceID, InstanceServerSummary, InstanceWorldSummary, WorldDatapackSummary},
    message::{AtomicBridgeDataLoadState, MessageToBackend, QuickPlayLaunch}, serial::AtomicOptionSerial,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme as _, IndexPath, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    list::{ListDelegate, ListItem, ListState},
    switch::Switch,
    v_flex,
};
use parking_lot::Mutex;
use rustc_hash::FxHashSet;

use crate::{entity::instance::InstanceEntry, icon::PandoraIcon, png_render_cache, root, ts};

pub struct InstanceQuickplaySubpage {
    instance: InstanceID,
    backend_handle: BackendHandle,
    worlds_state: Arc<AtomicBridgeDataLoadState>,
    world_list: Entity<ListState<WorldsListDelegate>>,
    servers_state: Arc<AtomicBridgeDataLoadState>,
    server_list: Entity<ListState<ServersListDelegate>>,
    worlds_serial: AtomicOptionSerial,
    servers_serial: AtomicOptionSerial,
}

impl InstanceQuickplaySubpage {
    pub fn new(
        instance: &Entity<InstanceEntry>,
        backend_handle: BackendHandle,
        mut window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let instance = instance.read(cx);
        let instance_id = instance.id;

        let worlds_state = Arc::clone(&instance.worlds_state);
        let servers_state = Arc::clone(&instance.servers_state);

        let worlds_list_delegate = WorldsListDelegate {
            id: instance_id,
            name: instance.name.clone(),
            backend_handle: backend_handle.clone(),
            worlds: instance.worlds.read(cx).to_vec(),
            searched: instance.worlds.read(cx).to_vec(),
            world_datapacks: instance.world_datapacks.clone(),
            expanded: Arc::new(AtomicUsize::new(0)),
            confirming_delete: Arc::new(Mutex::new(FxHashSet::default())),
        };

        let servers_list_delegate = ServersListDelegate {
            id: instance_id,
            name: instance.name.clone(),
            backend_handle: backend_handle.clone(),
            servers: instance.servers.read(cx).to_vec(),
            searched: instance.servers.read(cx).to_vec(),
        };

        let worlds = instance.worlds.clone();
        let servers = instance.servers.clone();
        let world_datapacks = instance.world_datapacks.clone();

        let window2 = &mut window;
        let world_list = cx.new(move |cx| {
            cx.observe(&worlds, |list: &mut ListState<WorldsListDelegate>, worlds, cx| {
                let worlds = worlds.read(cx).to_vec();
                let delegate = list.delegate_mut();
                delegate.worlds = worlds.clone();
                delegate.searched = worlds;
                cx.notify();
            }).detach();

            cx.observe(&world_datapacks, |_list: &mut ListState<WorldsListDelegate>, _, cx| {
                cx.notify();
            }).detach();

            ListState::new(worlds_list_delegate, window2, cx).selectable(false).searchable(true)
        });

        let server_list = cx.new(move |cx| {
            cx.observe(&servers, |list: &mut ListState<ServersListDelegate>, servers, cx| {
                let servers = servers.read(cx).to_vec();
                let delegate = list.delegate_mut();
                delegate.servers = servers.clone();
                delegate.searched = servers;
                cx.notify();
            }).detach();

            ListState::new(servers_list_delegate, window, cx).selectable(false).searchable(true)
        });

        Self {
            instance: instance_id,
            backend_handle,
            worlds_state,
            world_list,
            servers_state,
            server_list,
            worlds_serial: AtomicOptionSerial::default(),
            servers_serial: AtomicOptionSerial::default(),
        }
    }
}

impl Render for InstanceQuickplaySubpage {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let theme = cx.theme();

        let state = self.worlds_state.load(Ordering::SeqCst);
        if state.should_send_load_request() {
            self.backend_handle.send_with_serial(MessageToBackend::RequestLoadWorlds { id: self.instance }, &self.worlds_serial);
        }

        let state = self.servers_state.load(Ordering::SeqCst);
        if state.should_send_load_request() {
            self.backend_handle.send_with_serial(MessageToBackend::RequestLoadServers { id: self.instance }, &self.servers_serial);
        }

        let worlds_header = div().mb_1().ml_1().text_lg().child(ts!("instance.worlds"));
        let servers_header = div().mb_1().ml_1().text_lg().child(ts!("instance.servers"));

        v_flex().p_4().gap_4().size_full().child(
            h_flex()
                .size_full()
                .gap_4()
                .child(
                    v_flex().size_full().child(worlds_header).child(
                        v_flex()
                            .text_base()
                            .size_full()
                            .border_1()
                            .rounded(theme.radius)
                            .border_color(theme.border)
                            .child(self.world_list.clone()),
                    ),
                )
                .child(
                    v_flex().size_full().child(servers_header).child(
                        v_flex()
                            .text_base()
                            .size_full()
                            .border_1()
                            .rounded(theme.radius)
                            .border_color(theme.border)
                            .child(self.server_list.clone()),
                    ),
                ),
        )
    }
}

pub struct WorldsListDelegate {
    id: InstanceID,
    name: SharedString,
    backend_handle: BackendHandle,
    worlds: Vec<InstanceWorldSummary>,
    searched: Vec<InstanceWorldSummary>,
    world_datapacks: Entity<Arc<parking_lot::RwLock<rustc_hash::FxHashMap<String, Arc<[WorldDatapackSummary]>>>>>,
    expanded: Arc<AtomicUsize>,
    confirming_delete: Arc<Mutex<FxHashSet<u64>>>,
}

impl ListDelegate for WorldsListDelegate {
    type Item = ListItem;

    fn items_count(&self, _section: usize, cx: &App) -> usize {
        let expanded = self.expanded.load(Ordering::Relaxed);
        if expanded == 0 {
            return self.searched.len();
        }
        let world_folder = self
            .searched
            .get(expanded - 1)
            .and_then(|w| w.level_path.file_name().map(|n| n.to_string_lossy().to_string()));
        let extra = world_folder
            .and_then(|wf| {
                self.world_datapacks
                    .read(cx)
                    .read()
                    .get(&wf)
                    .map(|d| d.len())
            })
            .unwrap_or(0);
        self.searched.len() + extra
    }

    fn render_item(&mut self, ix: IndexPath, _window: &mut Window, cx: &mut Context<ListState<Self>>) -> Option<Self::Item> {
        let expanded = self.expanded.load(Ordering::Relaxed);
        let mut world_index = ix.row;

        if expanded > 0 {
            let world_folder = self
                .searched
                .get(expanded - 1)
                .and_then(|w| w.level_path.file_name().map(|n| n.to_string_lossy().to_string()));
            let num_children = world_folder
                .as_ref()
                .and_then(|wf| self.world_datapacks.read(cx).read().get(wf).map(|d| d.len()))
                .unwrap_or(0);

            if ix.row >= expanded && ix.row < expanded + num_children {
                let child_ix = ix.row - expanded;
                if let (Some(wf), Some(datapacks)) = (
                    world_folder.as_ref(),
                    world_folder.as_ref().and_then(|wf| self.world_datapacks.read(cx).read().get(wf).cloned()),
                ) {
                    if let Some(dp) = datapacks.get(child_ix) {
                        return Some(self.render_datapack(child_ix, dp, expanded - 1, wf, cx));
                    }
                }
            }
            if ix.row >= expanded {
                world_index = ix.row - num_children;
            }
        }

        let summary = self.searched.get(world_index)?;
        let world_folder = summary.level_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();

        let icon = if let Some(png_icon) = summary.png_icon.as_ref() {
            png_render_cache::render(Arc::clone(png_icon), cx)
        } else {
            gpui::img(ImageSource::Resource(Resource::Embedded("images/default_world.png".into())))
        };

        let description = v_flex().child(SharedString::from(summary.title.clone())).child(
            div()
                .text_color(Hsla {
                    h: 0.0,
                    s: 0.0,
                    l: 0.5,
                    a: 1.0,
                })
                .child(SharedString::from(summary.subtitle.clone())),
        );

        let id = self.id;
        let name = self.name.clone();
        let backend_handle = self.backend_handle.clone();
        let target = summary.level_path.file_name().unwrap().to_owned();

        let has_datapacks = summary.has_datapacks;
        let mut content = h_flex().gap_1();

        if has_datapacks {
            let expanded = self.expanded.clone();
            let world_idx = world_index + 1;
            let datapacks_entity = self.world_datapacks.clone();
            let backend_handle_req = self.backend_handle.clone();
            let expand_icon = if expanded.load(Ordering::Relaxed) == world_idx {
                PandoraIcon::ArrowDown
            } else {
                PandoraIcon::ArrowRight
            };
            content = content.child(
                Button::new(("expand_world", world_index))
                    .icon(expand_icon)
                    .compact()
                    .small()
                    .info()
                    .on_click(cx.listener(move |_this, _, _, cx| {
                        let cur = expanded.load(Ordering::Relaxed);
                        if cur == world_idx {
                            expanded.store(0, Ordering::Relaxed);
                        } else {
                            expanded.store(world_idx, Ordering::Relaxed);
                            let datapacks = datapacks_entity.read(cx).read().get(&world_folder).cloned();
                            if datapacks.is_none() {
                                backend_handle_req.send(MessageToBackend::RequestLoadWorldDatapacks {
                                    id,
                                    world_folder: world_folder.clone(),
                                });
                            }
                        }
                        cx.notify();
                    })),
            );
        }

        let item = ListItem::new(ix).p_1().child(
            content
                .child(
                    div().child(
                        Button::new(ix).success().icon(PandoraIcon::Play).on_click(move |_, window, cx| {
                            root::start_instance(
                                id,
                                name.clone(),
                                Some(QuickPlayLaunch::Singleplayer(target.clone())),
                                false,
                                &backend_handle,
                                window,
                                cx,
                            );
                        }),
                    ).px_2(),
                )
                .child(icon.size_16().min_w_16().min_h_16())
                .child(description),
        );

        Some(item)
    }

    fn set_selected_index(&mut self, _ix: Option<IndexPath>, _window: &mut Window, _cx: &mut Context<ListState<Self>>) {
    }

    fn perform_search(&mut self, query: &str, _window: &mut Window, _cx: &mut Context<ListState<Self>>) -> Task<()> {
        self.searched = self.worlds.iter().filter(|w| w.title.contains(query)).cloned().collect();

        Task::ready(())
    }
}

impl WorldsListDelegate {
    fn render_datapack(
        &self,
        child_ix: usize,
        dp: &WorldDatapackSummary,
        world_idx: usize,
        world_folder: &str,
        cx: &mut Context<ListState<Self>>,
    ) -> ListItem {
        let id = self.id;
        let backend_handle = self.backend_handle.clone();
        let world_folder = world_folder.to_string();
        let filename = dp.filename.to_string();
        let filename_display = filename.clone();
        let element_id = (world_idx as u64).wrapping_mul(31).wrapping_add(child_ix as u64);

        let confirming_delete = self.confirming_delete.clone();
        let delete_button = if confirming_delete.lock().contains(&element_id) {
            Button::new(("delete_dp", element_id)).danger().icon(PandoraIcon::Check).on_click(cx.listener({
                let backend_handle = backend_handle.clone();
                let confirming_delete = confirming_delete.clone();
                let world_folder = Arc::new(world_folder.clone());
                let filename = Arc::new(filename.clone());
                move |_this, _, _, cx| {
                    confirming_delete.lock().clear();
                    backend_handle.send(MessageToBackend::DeleteDatapack {
                        id,
                        world_folder: (*world_folder).clone(),
                        filename: (*filename).clone(),
                    });
                    cx.notify();
                }
            }))
        } else {
            Button::new(("delete_dp", element_id)).danger().icon(PandoraIcon::Trash2).on_click(cx.listener(move |_this, _, _, cx| {
                cx.stop_propagation();
                let mut guard = confirming_delete.lock();
                guard.clear();
                guard.insert(element_id);
                cx.notify();
            }))
        };

        let item_content = h_flex()
            .gap_1()
            .pl_4()
            .child(
                Switch::new(("toggle_dp", element_id))
                    .checked(dp.enabled)
                    .on_click(cx.listener({
                        let id = id;
                        let backend_handle = backend_handle.clone();
                        let world_folder = Arc::new(world_folder.clone());
                        let filename = Arc::new(filename);
                        move |_this, checked, _, cx| {
                            backend_handle.send(MessageToBackend::SetDatapackEnabled {
                                id,
                                world_folder: (*world_folder).clone(),
                                filename: (*filename).clone(),
                                enabled: *checked,
                            });
                            cx.notify();
                        }
                    }))
                    .px_2(),
            )
            .child(
                gpui::img(ImageSource::Resource(Resource::Embedded("images/default_mod.png".into())))
                    .size_16()
                    .min_w_16()
                    .min_h_16()
                    .grayscale(!dp.enabled),
            )
            .when(!dp.enabled, |this| this.line_through())
            .child(SharedString::from(filename_display))
            .child(delete_button.absolute().right_4());

        ListItem::new(("dp_item", element_id)).p_1().child(item_content)
    }
}

pub struct ServersListDelegate {
    id: InstanceID,
    name: SharedString,
    backend_handle: BackendHandle,
    servers: Vec<InstanceServerSummary>,
    searched: Vec<InstanceServerSummary>,
}

impl ListDelegate for ServersListDelegate {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.searched.len()
    }

    fn render_item(&mut self, ix: IndexPath, _window: &mut Window, cx: &mut Context<ListState<Self>>) -> Option<Self::Item> {
        let summary = self.searched.get(ix.row)?;

        let icon = if let Some(png_icon) = summary.png_icon.as_ref() {
            png_render_cache::render(Arc::clone(png_icon), cx)
        } else {
            gpui::img(ImageSource::Resource(Resource::Embedded("images/default_world.png".into())))
        };

        let description = v_flex()
            .child(SharedString::from(summary.name.clone()))
            .child(div().text_color(cx.theme().muted_foreground).child(SharedString::from(summary.ip.clone())));

        let id = self.id;
        let name = self.name.clone();
        let backend_handle = self.backend_handle.clone();
        let target = OsString::from(summary.ip.to_string());
        let item = ListItem::new(ix).p_1().child(
            h_flex()
                .gap_1()
                .child(
                    div()
                        .child(Button::new(ix).success().icon(PandoraIcon::Play).on_click(move |_, window, cx| {
                            root::start_instance(
                                id,
                                name.clone(),
                                Some(QuickPlayLaunch::Multiplayer(target.clone())),
                                false,
                                &backend_handle,
                                window,
                                cx,
                            );
                        }))
                        .px_2(),
                )
                .child(icon.size_16().min_w_16().min_h_16())
                .child(description),
        );

        Some(item)
    }

    fn set_selected_index(&mut self, _ix: Option<IndexPath>, _window: &mut Window, _cx: &mut Context<ListState<Self>>) {
    }

    fn perform_search(&mut self, query: &str, _window: &mut Window, _cx: &mut Context<ListState<Self>>) -> Task<()> {
        self.searched = self.servers.iter().filter(|w| w.name.contains(query)).cloned().collect();

        Task::ready(())
    }
}
