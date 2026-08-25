use std::collections::HashMap;

use bridge::{handle::BackendHandle, instance::InstanceID};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, IndexPath, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    menu::ContextMenuExt,
    select::{Select, SelectDelegate, SelectEvent, SelectItem, SelectState},
    table::{DataTable, TableState},
    v_flex,
};
use strum::IntoEnumIterator;

use crate::{
    component::{
        instance_list::InstanceList,
        named_dropdown::{NamedDropdown, NamedDropdownItem},
        responsive_grid::ResponsiveGrid,
    },
    entity::{
        DataEntities,
        instance::{InstanceAddedEvent, InstanceEntries},
        metadata::FrontendMetadata,
    },
    icon::PandoraIcon,
    interface_config::{InstanceGroup, InstancesViewMode, InterfaceConfig},
    modals,
    pages::page::Page,
};

#[derive(Clone)]
struct DragInstance {
    id: InstanceID,
    name: SharedString,
}

struct DragInstanceCard {
    name: SharedString,
}

impl Render for DragInstanceCard {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded_sm()
            .bg(gpui::black().opacity(0.8))
            .text_color(gpui::white())
            .text_sm()
            .child(self.name.clone())
    }
}

pub struct InstancesPage {
    instance_table: Entity<TableState<InstanceList>>,
    view_dropdown: Entity<SelectState<NamedDropdown<InstancesViewMode>>>,

    metadata: Entity<FrontendMetadata>,
    instances: Entity<InstanceEntries>,
    data: DataEntities,

    groups: Vec<InstanceGroup>,
    assignments: HashMap<InstanceID, u64>,
    pending_assignment: Option<u64>,

    backend_handle: BackendHandle,
}

impl InstancesPage {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let instance_table = InstanceList::create_table(data, window, cx);
        let view_dropdown = cx.new(|cx| {
            let items = InstancesViewMode::iter()
                .map(|view| NamedDropdownItem {
                    name: view.name(),
                    item: view,
                })
                .collect::<Vec<_>>();
            let current_view = InterfaceConfig::get(cx).instances_view_mode;
            let row = items.iter().position(|v| v.item == current_view).unwrap_or(0);
            let delegate = NamedDropdown::new(items);
            SelectState::new(delegate, Some(IndexPath::new(row)), window, cx)
        });
        cx.subscribe(&view_dropdown, |_, _, event: &SelectEvent<NamedDropdown<InstancesViewMode>>, cx| {
            let SelectEvent::Confirm(Some(view)) = event else {
                return;
            };
            InterfaceConfig::get_mut(cx).instances_view_mode = *view;
        })
        .detach();

        cx.subscribe::<_, InstanceAddedEvent>(&data.instances, |this, _, event, cx| {
            if let Some(group) = this.pending_assignment.take() {
                this.assignments.insert(event.instance.id, group);
                this.sync_groups_to_config(cx);
            }
            cx.notify();
        })
        .detach();

        let config = InterfaceConfig::get(cx);
        Self {
            instance_table,
            view_dropdown,
            metadata: data.metadata.clone(),
            instances: data.instances.clone(),
            data: data.clone(),
            groups: config.instance_groups.clone(),
            assignments: config.instance_group_assignments.clone(),
            pending_assignment: None,
            backend_handle: data.backend_handle.clone(),
        }
    }

    fn sync_groups_to_config(&self, cx: &mut App) {
        let config = InterfaceConfig::get_mut(cx);
        config.instance_groups = self.groups.clone();
        config.instance_group_assignments = self.assignments.clone();
    }

    fn new_group(&mut self, name: String, at: Option<usize>, cx: &mut Context<Self>) {
        let id = self.groups.iter().map(|g| g.id).max().unwrap_or(0) + 1;
        let group = InstanceGroup {
            id,
            name,
            collapsed: false,
        };
        match at {
            Some(index) => self.groups.insert(index.min(self.groups.len()), group),
            None => self.groups.push(group),
        };
        self.sync_groups_to_config(cx);
        cx.notify();
    }

    fn rename_group(&mut self, id: u64, name: String, cx: &mut Context<Self>) {
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == id) {
            group.name = name;
        }
        self.sync_groups_to_config(cx);
        cx.notify();
    }

    fn set_group_collapsed(&mut self, id: u64, collapsed: bool, cx: &mut Context<Self>) {
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == id) {
            group.collapsed = collapsed;
        }
        self.sync_groups_to_config(cx);
        cx.notify();
    }

    fn delete_group(&mut self, id: u64, cx: &mut Context<Self>) {
        self.groups.retain(|g| g.id != id);
        self.assignments.retain(|_, g| *g != id);
        self.sync_groups_to_config(cx);
        cx.notify();
    }

    fn assign_instance_group(&mut self, instance: InstanceID, group: Option<u64>, cx: &mut Context<Self>) {
        match group {
            Some(group) => {
                self.assignments.insert(instance, group);
            },
            None => {
                self.assignments.remove(&instance);
            },
        }
        self.sync_groups_to_config(cx);
        cx.notify();
    }

    fn open_new_group_dialog(this: &Entity<Self>, at: Option<usize>, window: &mut Window, cx: &mut App) {
        modals::rename_group::open_rename_group(
            "New group",
            String::new(),
            {
                let this = this.clone();
                move |name, _, cx| {
                    this.update(cx, |page, cx| page.new_group(name, at, cx));
                }
            },
            window,
            cx,
        );
    }

    fn render_group_header(&self, group: Option<&InstanceGroup>, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let (id, name, collapsed) = match group {
            Some(group) => (Some(group.id), SharedString::from(group.name.clone()), group.collapsed),
            None => (None, SharedString::from(t::instance::group::ungrouped()), false),
        };
        let theme = cx.theme();
        let has_groups = !self.groups.is_empty();
        let this = cx.entity();

        let name_element = if id.is_some() {
            let this_for_rename = this.clone();
            let id_for_rename = id;
            div()
                .id(("group-name", index))
                .cursor_pointer()
                .hover(|this| this.text_color(theme.link))
                .on_click(move |_, window, cx| {
                    let Some(id) = id_for_rename else { return };
                    let current_name = this_for_rename
                        .read(cx)
                        .groups
                        .iter()
                        .find(|g| g.id == id)
                        .map(|g| g.name.clone())
                        .unwrap_or_default();
                    modals::rename_group::open_rename_group(
                        "Rename group",
                        current_name,
                        {
                            let this_for_rename = this_for_rename.clone();
                            move |new_name, _, cx| {
                                this_for_rename.update(cx, |page, cx| page.rename_group(id, new_name, cx));
                            }
                        },
                        window,
                        cx,
                    );
                })
                .child(name)
                .into_any_element()
        } else {
            div().child(name).into_any_element()
        };

        let chevron = match collapsed {
            true => "icons/chevron-down.svg",
            false => "icons/chevron-up.svg",
        };

        let header = h_flex()
            .id(("group-header", index))
            .gap_2()
            .w_full()
            .items_center()
            .child(div().flex_1().h(px(1.0)).bg(theme.border))
            .child(name_element)
            .when(has_groups && id.is_some(), |header_elem| {
                let Some(gid) = id else { return header_elem };
                let this_for_toggle = this.clone();
                header_elem.child(
                    Button::new(("toggle-group", index))
                        .small()
                        .compact()
                        .ghost()
                        .icon(Icon::default().path(chevron))
                        .on_click(move |_, _, cx| {
                            let collapsed = this_for_toggle
                                .read(cx)
                                .groups
                                .iter()
                                .find(|g| g.id == gid)
                                .is_some_and(|g| g.collapsed);
                            this_for_toggle.update(cx, |page, cx| page.set_group_collapsed(gid, !collapsed, cx));
                        }),
                )
            })
            .child(div().flex_1().h(px(1.0)).bg(theme.border));

        if let Some(id) = id {
            header
                .context_menu(move |menu, _, _| {
                    menu.item(create_instance_menu_item(&this, id))
                        .separator()
                        .item(rename_menu_item(&this, id))
                        .separator()
                        .item(new_group_menu_item(&this, index - 1, t::instance::group::new_above()))
                        .item(new_group_menu_item(&this, index, t::instance::group::new_below()))
                        .separator()
                        .item(delete_menu_item(&this, id))
                })
                .into_any_element()
        } else {
            header.into_any_element()
        }
    }

    fn render_group_section(
        &self,
        group: Option<&InstanceGroup>,
        index: usize,
        cards: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = group.map(|g| g.collapsed).unwrap_or(false);
        let id = group.map(|g| g.id);
        let this = cx.entity();

        let size = Size::new(gpui::AvailableSpace::MinContent, gpui::AvailableSpace::MinContent);

        let mut section = v_flex()
            .id(("group-section", index))
            .gap_2()
            .p_1()
            .rounded_lg()
            .drag_over::<DragInstance>(|style, _, _, _| style.bg(gpui::white().opacity(0.05)))
            .on_drop(move |drag: &DragInstance, _, cx| {
                this.update(cx, |page, cx| page.assign_instance_group(drag.id, id, cx));
            });

        section = section.child(self.render_group_header(group, index, cx));

        if !collapsed {
            section = section.child(ResponsiveGrid::new(size).children(cards));
        }

        section.into_any_element()
    }

    fn render_cards(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let groups = self.groups.clone();
        let assignments = self.assignments.clone();
        let items = self.instance_table.read(cx).delegate().items.clone();

        let card_for = |index: usize, cx: &mut App| -> AnyElement {
            let entry = &items[index];
            let drag = DragInstance {
                id: entry.id,
                name: entry.name.clone(),
            };

            InstanceList::render_card(entry, index, &self.data, cx)
                .id(("instance-card", index))
                .cursor_grab()
                .on_drag(drag, |drag: &DragInstance, _, _, cx| {
                    cx.new(|_| DragInstanceCard {
                        name: drag.name.clone(),
                    })
                })
                .into_any_element()
        };

        let mut page = v_flex().p_4().gap_6();

        if groups.is_empty() {
            let cards = (0..items.len()).map(|i| card_for(i, cx)).collect::<Vec<_>>();
            let size = Size::new(gpui::AvailableSpace::MinContent, gpui::AvailableSpace::MinContent);
            return div()
                .p_4()
                .child(ResponsiveGrid::new(size).size_full().gap_4().children(cards))
                .into_any_element();
        }

        // Ungrouped instances section (implicit, always first).
        let ungrouped = items
            .iter()
            .enumerate()
            .filter(|(_, entry)| !assignments.contains_key(&entry.id))
            .map(|(i, _)| card_for(i, cx))
            .collect::<Vec<_>>();
        if !ungrouped.is_empty() {
            page = page.child(self.render_group_section(None, 0, ungrouped, cx));
        }

        for (group_index, group) in groups.iter().enumerate() {
            let cards = items
                .iter()
                .enumerate()
                .filter(|(_, entry)| assignments.get(&entry.id) == Some(&group.id))
                .map(|(i, _)| card_for(i, cx))
                .collect::<Vec<_>>();
            page = page.child(self.render_group_section(Some(group), group_index + 1, cards, cx));
        }

        page.into_any_element()
    }
}

impl Page for InstancesPage {
    fn controls(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let create_instance = Button::new("create_instance")
            .success()
            .icon(PandoraIcon::Plus)
            .label(t::instance::create())
            .on_click(cx.listener(|this, _, window, cx| {
                let entity = cx.entity();
                crate::modals::create_instance::open_create_instance(
                    this.metadata.clone(),
                    this.instances.clone(),
                    this.backend_handle.clone(),
                    None,
                    group_selection_handler(entity),
                    window,
                    cx,
                );
            }));
        // wrapping in div makes it not take up the full space of the titlebar
        let select_view =
            div().child(Select::new(&self.view_dropdown).title_prefix(format!("{}: ", t::instance::view_mode())));

        let new_group = Button::new("new_group")
            .icon(PandoraIcon::Plus)
            .label(t::instance::group::new())
            .on_click(cx.listener(|this, _, window, cx| {
                Self::open_new_group_dialog(&cx.entity(), None, window, cx);
            }));

        h_flex().gap_3().child(create_instance).child(new_group).child(select_view)
    }

    fn scrollable(&self, cx: &App) -> bool {
        match InterfaceConfig::get(cx).instances_view_mode {
            InstancesViewMode::Cards => true,
            InstancesViewMode::List => false,
        }
    }
}

impl Render for InstancesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match InterfaceConfig::get(cx).instances_view_mode {
            InstancesViewMode::Cards => self.render_cards(cx),
            InstancesViewMode::List => DataTable::new(&self.instance_table).bordered(false).into_any_element(),
        }
    }
}

fn group_selection_handler(
    this: Entity<InstancesPage>,
) -> impl Fn(modals::select_group::GroupSelection, &mut Window, &mut App) {
    move |selection, _, cx| {
        this.update(cx, |page, cx| match selection {
            modals::select_group::GroupSelection::Existing(group) => {
                page.pending_assignment = group.id;
            },
            modals::select_group::GroupSelection::New(name) => {
                page.new_group(name, None, cx);
                page.pending_assignment = page.groups.last().map(|g| g.id);
            },
        });
    }
}

fn create_instance_menu_item(this: &Entity<InstancesPage>, group_id: u64) -> gpui_component::menu::PopupMenuItem {
    let this = this.clone();
    gpui_component::menu::PopupMenuItem::new(t::instance::create()).on_click(move |_, window, cx| {
        let (metadata, instances, backend_handle, group_name) = {
            let page = this.read(cx);
            let group_name = page.groups.iter().find(|g| g.id == group_id).map(|g| g.name.clone()).unwrap_or_default();
            (page.metadata.clone(), page.instances.clone(), page.backend_handle.clone(), group_name)
        };
        let preselected = modals::select_group::SelectedGroup {
            id: Some(group_id),
            name: group_name.into(),
        };
        this.update(cx, |page, _| page.pending_assignment = Some(group_id));
        modals::create_instance::open_create_instance(
            metadata,
            instances,
            backend_handle,
            Some(preselected),
            group_selection_handler(this.clone()),
            window,
            cx,
        );
    })
}

fn rename_menu_item(this: &Entity<InstancesPage>, id: u64) -> gpui_component::menu::PopupMenuItem {
    let this = this.clone();
    gpui_component::menu::PopupMenuItem::new(t::instance::group::rename()).on_click(move |_, window, cx| {
        let current_name = this
            .read(cx)
            .groups
            .iter()
            .find(|g| g.id == id)
            .map(|g| g.name.clone())
            .unwrap_or_default();
        modals::rename_group::open_rename_group(
            "Rename group",
            current_name,
            {
                let this = this.clone();
                move |new_name, _, cx| {
                    this.update(cx, |page, cx| page.rename_group(id, new_name, cx));
                }
            },
            window,
            cx,
        );
    })
}

fn new_group_menu_item(
    this: &Entity<InstancesPage>,
    at: usize,
    label: &'static str,
) -> gpui_component::menu::PopupMenuItem {
    let this = this.clone();
    gpui_component::menu::PopupMenuItem::new(label).on_click(move |_, window, cx| {
        InstancesPage::open_new_group_dialog(&this, Some(at), window, cx);
    })
}

fn delete_menu_item(this: &Entity<InstancesPage>, id: u64) -> gpui_component::menu::PopupMenuItem {
    let this = this.clone();
    gpui_component::menu::PopupMenuItem::new(t::instance::group::delete()).on_click(move |_, _, cx| {
        this.update(cx, |page, cx| page.delete_group(id, cx));
    })
}

#[derive(Default)]
pub struct VersionList {
    pub versions: Vec<SharedString>,
    pub matched_versions: Vec<SharedString>,
}

impl SelectDelegate for VersionList {
    type Item = SharedString;

    fn items_count(&self, _section: usize) -> usize {
        self.matched_versions.len()
    }

    fn item(&self, ix: IndexPath) -> Option<&Self::Item> {
        self.matched_versions.get(ix.row)
    }

    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        Self::Item: gpui_component::select::SelectItem<Value = V>,
        V: PartialEq,
    {
        for (ix, item) in self.matched_versions.iter().enumerate() {
            if item.value() == value {
                return Some(IndexPath::default().row(ix));
            }
        }

        None
    }

    fn perform_search(&mut self, query: &str, _window: &mut Window, _: &mut App) -> Task<()> {
        let lower_query = query.to_lowercase();

        self.matched_versions = self
            .versions
            .iter()
            .filter(|item| item.to_lowercase().starts_with(&lower_query))
            .cloned()
            .collect();

        Task::ready(())
    }
}
