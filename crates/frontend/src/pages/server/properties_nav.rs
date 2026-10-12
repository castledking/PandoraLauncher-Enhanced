use bridge::handle::BackendHandle;
use gpui::{prelude::*, *};
use gpui_component::{
    Icon,
    tab::{Tab, TabBar},
    v_flex,
};
use schema::server_list::ServerListFile;

use crate::{
    entity::instance::InstanceEntry,
    icon::PandoraIcon,
    pages::server::{list_subpage::ServerListSubpage, properties_subpage::ServerPropertiesSubpage},
};

/// The pages reachable from the Properties tab.
///
/// A server keeps a handful of small JSON files of its own next to server.properties: who
/// is banned, who is let in, who runs the place. They live under Properties rather than
/// getting tabs of their own on the page, since there are six of them and none is the
/// kind of thing anyone goes looking for on its own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PropertiesTab {
    Properties,
    List(ServerListFile),
}

/// Every tab in order, which is also the order they appear in.
const TABS: [PropertiesTab; 6] = [
    PropertiesTab::Properties,
    PropertiesTab::List(ServerListFile::BannedPlayers),
    PropertiesTab::List(ServerListFile::BannedIps),
    PropertiesTab::List(ServerListFile::Whitelist),
    PropertiesTab::List(ServerListFile::Operators),
    PropertiesTab::List(ServerListFile::UserCache),
];

#[derive(Clone)]
enum PropertiesBody {
    Server(Entity<ServerPropertiesSubpage>),
    List(Entity<ServerListSubpage>),
}

impl PropertiesBody {
    fn into_any_element(self) -> AnyElement {
        match self {
            PropertiesBody::Server(entity) => entity.into_any_element(),
            PropertiesBody::List(entity) => entity.into_any_element(),
        }
    }
}

/// Wraps server.properties and the server's own lists behind a second row of tabs.
pub struct ServerPropertiesNav {
    instance: Entity<InstanceEntry>,
    backend_handle: BackendHandle,
    current: PropertiesTab,
    body: PropertiesBody,
}

impl ServerPropertiesNav {
    pub fn new(
        instance: &Entity<InstanceEntry>,
        backend_handle: BackendHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let properties = cx.new(|cx| ServerPropertiesSubpage::new(instance, backend_handle.clone(), window, cx));

        Self {
            instance: instance.clone(),
            backend_handle,
            current: PropertiesTab::Properties,
            body: PropertiesBody::Server(properties),
        }
    }

    fn switch(&mut self, tab: PropertiesTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.current == tab {
            return;
        }

        let instance = &self.instance;
        let backend_handle = &self.backend_handle;

        self.body = match tab {
            PropertiesTab::Properties => PropertiesBody::Server(
                cx.new(|cx| ServerPropertiesSubpage::new(instance, backend_handle.clone(), window, cx)),
            ),
            PropertiesTab::List(file) => PropertiesBody::List(
                cx.new(|cx| ServerListSubpage::new(instance, backend_handle.clone(), file, window, cx)),
            ),
        };
        self.current = tab;
        cx.notify();
    }
}

impl Render for ServerPropertiesNav {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let labels = TABS.map(|tab| match tab {
            PropertiesTab::Properties => (t::server::tab::properties(), PandoraIcon::SlidersVertical),
            PropertiesTab::List(ServerListFile::BannedPlayers) => {
                (t::server::lists::banned_players(), PandoraIcon::CircleX)
            },
            PropertiesTab::List(ServerListFile::BannedIps) => (t::server::lists::banned_ips(), PandoraIcon::Globe),
            PropertiesTab::List(ServerListFile::Whitelist) => (t::server::lists::whitelist(), PandoraIcon::CircleCheck),
            PropertiesTab::List(ServerListFile::Operators) => (t::server::lists::operators(), PandoraIcon::Award),
            PropertiesTab::List(ServerListFile::UserCache) => (t::server::lists::user_cache(), PandoraIcon::Users),
        });

        let selected_index = TABS.iter().position(|tab| *tab == self.current).unwrap_or(0);

        let tab_bar = TabBar::new("properties-tabs")
            .underline()
            .selected_index(selected_index)
            .children(labels.map(|(label, icon)| Tab::new().label(label).prefix(Icon::new(icon).size_4().ml_2())))
            .on_click(cx.listener(|this, index: &usize, window, cx| {
                if let Some(tab) = TABS.get(*index).copied() {
                    this.switch(tab, window, cx);
                }
            }));

        v_flex()
            .size_full()
            .child(tab_bar)
            .child(div().flex_1().min_h_0().child(self.body.clone().into_any_element()))
    }
}
