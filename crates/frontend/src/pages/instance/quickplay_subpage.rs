use std::{ffi::OsString, sync::{atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering}, Arc, RwLock}};

use bridge::{handle::BackendHandle, instance::{InstanceID, InstanceServerSummary, InstanceWorldSummary}, message::{AtomicBridgeDataLoadState, MessageToBackend, QuickPlayLaunch}};
use gpui::{prelude::*, *};
use gpui_component::{
    alert::Alert, button::{Button, ButtonGroup, ButtonVariants}, checkbox::Checkbox, form::form_field, group_box::GroupBox, h_flex, input::{Input, InputEvent, InputState}, list::{List, ListDelegate, ListItem, ListState}, resizable::{h_resizable, resizable_panel, ResizableState}, select::{SearchableVec, Select, SelectDelegate, SelectItem, SelectState}, sidebar::{Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem}, skeleton::Skeleton, tab::{Tab, TabBar}, table::{Column, ColumnFixed, ColumnSort, Table, TableDelegate}, v_flex, ActiveTheme as _, Icon, IconName, IndexPath, Root, Selectable, Sizable, StyledExt
};

use crate::{entity::instance::InstanceEntry, png_render_cache, root};

pub struct InstanceQuickplaySubpage {
    instance: InstanceID,
    backend_handle: BackendHandle,
    worlds_state: Arc<AtomicBridgeDataLoadState>,
    world_list: Entity<ListState<WorldsListDelegate>>,
    servers_state: Arc<AtomicBridgeDataLoadState>,
    server_list: Entity<ListState<ServersListDelegate>>,
}

impl InstanceQuickplaySubpage {
    pub fn new(instance: &Entity<InstanceEntry>, backend_handle: BackendHandle, mut window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> Self {
        let instance = instance.read(cx);
        let instance_id = instance.id;
        
        let worlds_state = Arc::clone(&instance.worlds_state);
        let servers_state = Arc::clone(&instance.servers_state);
        
        let worlds_list_delegate = WorldsListDelegate {
            id: instance_id,
            name: instance.name.clone(),
            backend_handle: backend_handle.clone(),
            worlds: (&*instance.worlds.read(cx)).to_vec(),
            searched: (&*instance.worlds.read(cx)).to_vec(),
        };
        
        let servers_list_delegate = ServersListDelegate {
            id: instance_id,
            name: instance.name.clone(),
            backend_handle: backend_handle.clone(),
            servers: (&*instance.servers.read(cx)).to_vec(),
            searched: (&*instance.servers.read(cx)).to_vec(),
        };
        
        let worlds = instance.worlds.clone();
        let servers = instance.servers.clone();
        
        let window2 = &mut window;
        let world_list = cx.new(move |cx| {
            cx.observe(&worlds, |list: &mut ListState<WorldsListDelegate>, worlds, cx| {
                let worlds = (&*worlds.read(cx)).to_vec();
                let delegate = list.delegate_mut();
                delegate.worlds = worlds.clone();
                delegate.searched = worlds;
                cx.notify();
            }).detach();
            
            ListState::new(worlds_list_delegate, window2, cx).selectable(false).searchable(true)
        });
        
        let server_list = cx.new(move |cx| {
            cx.observe(&servers, |list: &mut ListState<ServersListDelegate>, servers, cx| {
                let servers = (&*servers.read(cx)).to_vec();
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
        }
    }
}

impl Render for InstanceQuickplaySubpage {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let theme = cx.theme();
        
        let state = self.worlds_state.load(Ordering::SeqCst);
        if state.should_send_load_request() {
            self.backend_handle.blocking_send(MessageToBackend::RequestLoadWorlds { id: self.instance });
        }
        
        let state = self.servers_state.load(Ordering::SeqCst);
        if state.should_send_load_request() {
            self.backend_handle.blocking_send(MessageToBackend::RequestLoadServers { id: self.instance });
        }
        
        v_flex()
            .p_4()
            .gap_4()
            .size_full()
            .child(h_flex()
                .size_full()
                .gap_4()
                .child(v_flex().size_full().text_lg().child("Worlds")
                    .child(v_flex().text_base().size_full().border_1().rounded(theme.radius).border_color(theme.border)
                        .child(self.world_list.clone())))
                .child(v_flex().size_full().text_lg().child("Servers")
                    .child(v_flex().text_base().size_full().border_1().rounded(theme.radius).border_color(theme.border)
                        .child(self.server_list.clone())))
            )
    }
}

pub struct WorldsListDelegate {
    id: InstanceID,
    name: SharedString,
    backend_handle: BackendHandle,
    worlds: Vec<InstanceWorldSummary>,
    searched: Vec<InstanceWorldSummary>,
}

impl ListDelegate for WorldsListDelegate {
    type Item = ListItem;

    fn items_count(&self, section: usize, cx: &App) -> usize {
        self.searched.len()
    }

    fn render_item(
        &self,
        ix: IndexPath,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Self::Item> {
        let summary = self.searched.get(ix.row)?;
        
        let icon = if let Some(png_icon) = summary.png_icon.as_ref() {
            png_render_cache::render(Arc::clone(png_icon), cx)
        } else {
            gpui::img(ImageSource::Resource(Resource::Embedded("images/default_world.png".into())))
        };
        
        let description = v_flex()
            .child(SharedString::from(summary.title.clone()))
            .child(div().text_color(Hsla { h: 0.0, s: 0.0, l: 0.5, a: 1.0}).child(SharedString::from(summary.subtitle.clone())));
        
        let play_icon = Icon::empty().path("icons/play.svg");
        
        let id = self.id;
        let name = self.name.clone();
        let backend_handle = self.backend_handle.clone();
        let target = summary.level_path.file_name().unwrap().to_owned();
        let item = ListItem::new(ix)
            .p_1()
            .child(h_flex()
                .gap_1()
                .child(div().child(Button::new(ix).success().icon(play_icon).on_click(move |_, window, cx| {
                    root::start_instance(id, name.clone(), Some(QuickPlayLaunch::Singleplayer(target.clone())), &backend_handle, window, cx);
                })).px_2())
                .child(icon.size_16().min_w_16().min_h_16())
                .child(description)
            );
        
        Some(item)
    }
    
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
    }
    
    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.searched = self.worlds.iter()
            .filter(|w| w.title.contains(query))
            .cloned()
            .collect();
        
        Task::ready(())
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

    fn items_count(&self, section: usize, cx: &App) -> usize {
        self.searched.len()
    }

    fn render_item(
        &self,
        ix: IndexPath,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Self::Item> {
        let summary = self.searched.get(ix.row)?;
        
        let icon = if let Some(png_icon) = summary.png_icon.as_ref() {
            png_render_cache::render(Arc::clone(png_icon), cx)
        } else {
            gpui::img(ImageSource::Resource(Resource::Embedded("images/default_world.png".into())))
        };
        
        let description = v_flex()
            .child(SharedString::from(summary.name.clone()))
            .child(div().text_color(Hsla { h: 0.0, s: 0.0, l: 0.5, a: 1.0}).child(SharedString::from(summary.ip.clone())));
        
        let play_icon = Icon::empty().path("icons/play.svg");
        
        let id = self.id;
        let name = self.name.clone();
        let backend_handle = self.backend_handle.clone();
        let target = OsString::from(summary.ip.to_string());
        let item = ListItem::new(ix)
            .p_1()
            .child(h_flex()
                .gap_1()
                .child(div().child(Button::new(ix).success().icon(play_icon).on_click(move |_, window, cx| {
                    root::start_instance(id, name.clone(), Some(QuickPlayLaunch::Multiplayer(target.clone())), &backend_handle, window, cx);
                })).px_2())
                .child(icon.size_16().min_w_16().min_h_16())
                .child(description)
            );
        
        Some(item)
    }
    
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
    }
    
    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.searched = self.servers.iter()
            .filter(|w| w.name.contains(query))
            .cloned()
            .collect();
        
        Task::ready(())
    }
}
