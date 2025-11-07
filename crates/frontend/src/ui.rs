use std::sync::Arc;

use bridge::instance::InstanceID;
use gpui::{prelude::*, *};
use gpui_component::{
    h_flex, resizable::{h_resizable, resizable_panel, ResizableState}, sidebar::{Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem}, v_flex, ActiveTheme as _, Icon, IconName
};

use crate::{entity::{instance::{InstanceAddedEvent, InstanceEntry, InstanceModifiedEvent}, DataEntities}, pages::{instance::instance_page::InstancePage, instances_page::InstancesPage, modrinth_page::ModrinthSearchPage}, png_render_cache};

pub struct LauncherUI {
    data: DataEntities,
    page: LauncherPage,
    sidebar_state: Entity<ResizableState>,
    recent_instances: heapless::Vec<(InstanceID, SharedString), 3>,
    _instance_added_subscription: Subscription,
    _instance_modified_subscription: Subscription,
}

#[derive(Clone)]
pub enum LauncherPage {
    Instances(Entity<InstancesPage>),
    Modrinth(Entity<ModrinthSearchPage>),
    InstancePage(InstanceID, Entity<InstancePage>),
}

impl LauncherPage {
    pub fn into_any_element(self) -> AnyElement {
        match self {
            LauncherPage::Instances(entity) => entity.into_any_element(),
            LauncherPage::Modrinth(entity) => entity.into_any_element(),
            LauncherPage::InstancePage(_, entity) => entity.into_any_element(),
        }
    }
    
    pub fn page_type(&self) -> PageType {
        match self {
            LauncherPage::Instances(_) => PageType::Instances,
            LauncherPage::Modrinth(_) => PageType::Modrinth,
            LauncherPage::InstancePage(id, _) => PageType::InstancePage(*id),
        }
    }
}

impl LauncherUI {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let instance_page = cx.new(|cx| InstancesPage::new(data, window, cx));
        let sidebar_state = cx.new(|_| ResizableState::default());
        
        let recent_instances = data.instances.read(cx).entries.iter().take(3)
            .map(|(id, ent)| (*id, ent.read(cx).name.clone())).collect();
        
        let _instance_added_subscription = cx.subscribe::<_, InstanceAddedEvent>(&data.instances, |this, _, event, cx| {
            if this.recent_instances.is_full() {
                this.recent_instances.pop();
            }
            let _ = this.recent_instances.insert(0, (event.instance.id, event.instance.name.clone()));
            cx.notify();
        });
        let _instance_modified_subscription = cx.subscribe::<_, InstanceModifiedEvent>(&data.instances, |this, _, event, cx| {
            if let Some((_, name)) = this.recent_instances.iter_mut().find(|(id, _)| *id == event.instance.id) {
                *name = event.instance.name.clone();
                cx.notify();
            }
            cx.notify();
        });
        
        Self {
            data: data.clone(),
            page: LauncherPage::Instances(instance_page),
            sidebar_state,
            recent_instances,
            _instance_added_subscription,
            _instance_modified_subscription,
        }
    }

    pub fn switch_page(&mut self, page: PageType, window: &mut Window, cx: &mut Context<Self>) {
        let data = &self.data;
        match page {
            PageType::Instances => {
                if let LauncherPage::Instances(..) = self.page {
                    return;
                }
                self.page = LauncherPage::Instances(cx.new(|cx| InstancesPage::new(data, window, cx)));
                cx.notify();
            },
            PageType::Modrinth => {
                if let LauncherPage::Modrinth(..) = self.page {
                    return;
                }
                self.page = LauncherPage::Modrinth(cx.new(|cx| ModrinthSearchPage::new(data, window, cx)));
                cx.notify();
            },
            PageType::InstancePage(id) => {
                if let LauncherPage::InstancePage(current_id, ..) = self.page && current_id == id {
                    return;
                }
                self.page = LauncherPage::InstancePage(id, cx.new(|cx| InstancePage::new(id, data, window, cx)));
                cx.notify();
            },
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum PageType {
    Instances,
    Modrinth,
    InstancePage(InstanceID),
}

impl Render for LauncherUI {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page_type = self.page.page_type();
        
        let library_group = SidebarGroup::new("Library").child(SidebarMenu::new().children([
            SidebarMenuItem::new("Instances").active(page_type == PageType::Instances).on_click(cx.listener(|launcher, _, window, cx| {
                launcher.switch_page(PageType::Instances, window, cx);
            })),
            SidebarMenuItem::new("Mods"),
            SidebarMenuItem::new("Worlds"),
        ]));
        
        let launcher_group = SidebarGroup::new("Content").child(SidebarMenu::new().children([
            SidebarMenuItem::new("Modrinth").active(page_type == PageType::Modrinth).on_click(cx.listener(|launcher, _, window, cx| {
                launcher.switch_page(PageType::Modrinth, window, cx);
            })),
        ]));
        
        let mut groups: heapless::Vec<SidebarGroup<SidebarMenu>, 3> = heapless::Vec::new();
       
        let _ = groups.push(library_group);
        let _ = groups.push(launcher_group);
        
        if !self.recent_instances.is_empty() {
            let recent_instances_group = SidebarGroup::new("Recent Instances").child(SidebarMenu::new().children(
                self.recent_instances.iter().map(|(id, name)| {
                    let name = name.clone();
                    let id = *id;
                    SidebarMenuItem::new(name).active(page_type == PageType::InstancePage(id)).on_click(cx.listener(move |launcher, _, window, cx| {
                        launcher.switch_page(PageType::InstancePage(id), window, cx);
                    }))
                })
            ));
            let _ = groups.push(recent_instances_group);
        }
        
        let accounts = self.data.accounts.read(cx);
        let (account_head, account_name) = if let Some(account) = &accounts.selected_account {
            let account_name = SharedString::new(account.username.clone());
            let head = if let Some(head) = &account.head {
                png_render_cache::render(Arc::clone(head), cx)
            } else {
                gpui::img(ImageSource::Resource(Resource::Embedded("images/default_head.png".into())))
            };
            (head, account_name)
        } else {
            (gpui::img(ImageSource::Resource(Resource::Embedded("images/default_head.png".into()))), "No Account".into())
        };
        
        let pandora_icon = Icon::empty().path("icons/pandora.svg");
        
        let sidebar = Sidebar::left()
            .width(relative(1.))
            .border_width(px(0.))
            .header(
                SidebarHeader::new()
                    .w_full()
                    .justify_center()
                    .text_size(rems(0.9375))
                    .child(pandora_icon.size_8().min_w_8().min_h_8())
                    .child("Pandora")
            )
            .footer(
                SidebarFooter::new()
                    .w_full()
                    .justify_center()
                    .text_size(rems(0.9375))
                    .child(account_head.size_8().min_w_8().min_h_8())
                    .child(account_name)
            )
            .children(groups);

        h_resizable("container").with_state(&self.sidebar_state)
            .child(
                resizable_panel()
                    .size(px(150.))
                    .size_range(px(125.)..px(200.))
                    .child(sidebar),
            )
            .child(self.page.clone().into_any_element())
    }
}

pub fn page(cx: &App, title: impl IntoElement) -> gpui::Div {
    v_flex()
        .size_full()
        .child(
            h_flex()
                .p_4()
                .border_b_1()
                .border_color(cx.theme().border)
                .text_xl()
                .child(div().left_4().child(title)),
        )
}
