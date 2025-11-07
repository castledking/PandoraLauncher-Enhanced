use std::{ffi::OsString, sync::{atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering}, Arc, RwLock}};

use bridge::{handle::BackendHandle, instance::{InstanceID, InstanceModSummary, InstanceServerSummary, InstanceWorldSummary}, message::{AtomicBridgeDataLoadState, MessageToBackend, QuickPlayLaunch}};
use gpui::{prelude::*, *};
use gpui_component::{
    alert::Alert, button::{Button, ButtonGroup, ButtonVariants}, checkbox::Checkbox, select::{Select, SelectDelegate, SelectItem, SelectState, SearchableVec}, form::form_field, group_box::GroupBox, h_flex, input::{InputEvent, InputState, Input}, resizable::{h_resizable, resizable_panel, ResizableState}, sidebar::{Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem}, skeleton::Skeleton, switch::Switch, tab::{Tab, TabBar}, table::{Column, ColumnFixed, ColumnSort, Table, TableDelegate}, v_flex, ActiveTheme as _, Icon, IconName, IndexPath, list::{List, ListDelegate, ListItem, ListState}, Root, Selectable, Sizable, StyledExt
};

use crate::{entity::instance::InstanceEntry, png_render_cache, root};

pub struct InstanceModsSubpage {
    instance: InstanceID,
    backend_handle: BackendHandle,
    mods_state: Arc<AtomicBridgeDataLoadState>,
    mod_list: Entity<ListState<ModsListDelegate>>,
}

impl InstanceModsSubpage {
    pub fn new(instance: &Entity<InstanceEntry>, backend_handle: BackendHandle, mut window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> Self {
        let instance = instance.read(cx);
        let instance_id = instance.id;
        
        let mods_state = Arc::clone(&instance.mods_state);
        
        let mods_list_delegate = ModsListDelegate {
            id: instance_id,
            name: instance.name.clone(),
            backend_handle: backend_handle.clone(),
            mods: (&*instance.mods.read(cx)).to_vec(),
            searched: (&*instance.mods.read(cx)).to_vec(),
        };
        
        let mods = instance.mods.clone();
        
        let mod_list = cx.new(move |cx| {
            cx.observe(&mods, |list: &mut ListState<ModsListDelegate>, mods, cx| {
                let mods = (&*mods.read(cx)).to_vec();
                let delegate = list.delegate_mut();
                delegate.mods = mods.clone();
                delegate.searched = mods;
                cx.notify();
            }).detach();
            
            ListState::new(mods_list_delegate, window, cx).selectable(false).searchable(true)
        });
        
        Self {
            instance: instance_id,
            backend_handle,
            mods_state,
            mod_list,
        }
    }
}

impl Render for InstanceModsSubpage {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let theme = cx.theme();
        
        let state = self.mods_state.load(Ordering::SeqCst);
        if state.should_send_load_request() {
            self.backend_handle.blocking_send(MessageToBackend::RequestLoadMods { id: self.instance });
        }
        
        v_flex()
            .p_4()
            .gap_4()
            .size_full()
            .child(h_flex()
                .size_full()
                .gap_4()
                .child(v_flex().size_full().text_lg().child("Mods")
                    .child(v_flex().text_base().size_full().border_1().rounded(theme.radius).border_color(theme.border)
                        .child(self.mod_list.clone())))
            )
    }
}

pub struct ModsListDelegate {
    id: InstanceID,
    name: SharedString,
    backend_handle: BackendHandle,
    mods: Vec<InstanceModSummary>,
    searched: Vec<InstanceModSummary>,
}

impl ListDelegate for ModsListDelegate {
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
        
        let icon = if let Some(png_icon) = summary.mod_summary.png_icon.as_ref() {
            png_render_cache::render(Arc::clone(png_icon), cx)
        } else {
            // todo: empty mod icon?
            gpui::img(ImageSource::Resource(Resource::Embedded("images/default_world.png".into())))
        };
        
        const GRAY: Hsla = Hsla { h: 0.0, s: 0.0, l: 0.5, a: 1.0};
        
        let description1 = v_flex()
            .w_1_5()
            .text_ellipsis()
            .child(SharedString::from(summary.mod_summary.name.clone()))
            .child(SharedString::from(summary.mod_summary.version_str.clone()));
        
        let description2 = v_flex()
            .text_color(GRAY)
            .child(SharedString::from(summary.mod_summary.authors.clone()))
            .child(SharedString::from(summary.file_name.clone()));
        
        let id = self.id;
        let mod_id = summary.id;
        let backend_handle = self.backend_handle.clone();
        let item = ListItem::new(ix)
            .p_1()
            .child(h_flex()
                .gap_1()
                .child(Switch::new(ix).checked(summary.enabled).on_click(move |checked, window, cx| {
                    backend_handle.blocking_send(MessageToBackend::SetModEnabled {
                        id,
                        mod_id,
                        enabled: *checked
                    });
                }).px_2())
                .child(icon.size_16().min_w_16().min_h_16().grayscale(!summary.enabled))
                .when(!summary.enabled, |this| this.line_through())
                .child(description1)
                .child(description2)
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
        self.searched = self.mods.iter()
            .filter(|m| m.mod_summary.name.contains(query) || m.mod_summary.id.contains(query))
            .cloned()
            .collect();
        
        Task::ready(())
    }
}
