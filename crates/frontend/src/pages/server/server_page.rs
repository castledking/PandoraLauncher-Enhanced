use bridge::{instance::InstanceID, message::MessageToBackend, modal_action::ModalAction};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    tab::{Tab, TabBar},
    v_flex,
};
use schema::server::ServerPlatform;

use crate::{
    entity::{DataEntities, instance::InstanceEntry},
    icon::PandoraIcon,
    pages::{
        instance::content_subpage::{ContentType, InstanceContentSubpage},
        page::Page,
        server::{
            console_subpage::ServerConsoleSubpage, properties_subpage::ServerPropertiesSubpage,
            settings_subpage::ServerSettingsSubpage,
        },
        servers_page::{build_label, platform_icon, power_button, status_pill},
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ServerTab {
    Console,
    Content,
    Properties,
    Settings,
}

#[derive(Clone)]
enum ServerSubpage {
    Console(Entity<ServerConsoleSubpage>),
    Content(Entity<InstanceContentSubpage>),
    Properties(Entity<ServerPropertiesSubpage>),
    Settings(Entity<ServerSettingsSubpage>),
}

impl ServerSubpage {
    fn tab(&self) -> ServerTab {
        match self {
            ServerSubpage::Console(_) => ServerTab::Console,
            ServerSubpage::Content(_) => ServerTab::Content,
            ServerSubpage::Properties(_) => ServerTab::Properties,
            ServerSubpage::Settings(_) => ServerTab::Settings,
        }
    }

    fn into_any_element(self) -> AnyElement {
        match self {
            ServerSubpage::Console(entity) => entity.into_any_element(),
            ServerSubpage::Content(entity) => entity.into_any_element(),
            ServerSubpage::Properties(entity) => entity.into_any_element(),
            ServerSubpage::Settings(entity) => entity.into_any_element(),
        }
    }
}

pub struct ServerPage {
    data: DataEntities,
    pub instance: Entity<InstanceEntry>,
    subpage: ServerSubpage,
    _observe_instance: Subscription,
}

impl ServerPage {
    pub fn new(id: InstanceID, data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let instance = data.instances.read(cx).entries.get(&id).unwrap().clone();
        let subpage = Self::create_subpage(ServerTab::Console, &instance, data, window, cx);
        let observe_instance = cx.observe(&instance, |_, _, cx| cx.notify());

        Self {
            data: data.clone(),
            instance,
            subpage,
            _observe_instance: observe_instance,
        }
    }

    fn platform(&self, cx: &App) -> ServerPlatform {
        self.instance
            .read(cx)
            .configuration
            .server
            .as_ref()
            .map(|server| server.platform)
            .unwrap_or_default()
    }

    /// Plugin platforms get a Plugins tab, modded ones a Mods tab, and Vanilla neither.
    fn content_type(platform: ServerPlatform) -> Option<ContentType> {
        if platform.uses_plugins() {
            Some(ContentType::Plugins)
        } else if platform.uses_mods() {
            Some(ContentType::Mods)
        } else {
            None
        }
    }

    fn create_subpage(
        tab: ServerTab,
        instance: &Entity<InstanceEntry>,
        data: &DataEntities,
        window: &mut Window,
        cx: &mut App,
    ) -> ServerSubpage {
        let backend_handle = data.backend_handle.clone();
        match tab {
            ServerTab::Console => {
                ServerSubpage::Console(cx.new(|cx| ServerConsoleSubpage::new(instance, backend_handle, window, cx)))
            },
            ServerTab::Content => {
                let platform = instance
                    .read(cx)
                    .configuration
                    .server
                    .as_ref()
                    .map(|server| server.platform)
                    .unwrap_or_default();
                match Self::content_type(platform) {
                    Some(content_type) => ServerSubpage::Content(cx.new(|cx| {
                        InstanceContentSubpage::new(instance, content_type, data, backend_handle, window, cx)
                    })),
                    None => Self::create_subpage(ServerTab::Console, instance, data, window, cx),
                }
            },
            ServerTab::Properties => ServerSubpage::Properties(
                cx.new(|cx| ServerPropertiesSubpage::new(instance, backend_handle, window, cx)),
            ),
            ServerTab::Settings => {
                ServerSubpage::Settings(cx.new(|cx| ServerSettingsSubpage::new(instance, data, window, cx)))
            },
        }
    }

    fn switch_tab(&mut self, tab: ServerTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.subpage.tab() == tab {
            return;
        }
        self.subpage = Self::create_subpage(tab, &self.instance, &self.data, window, cx);
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entry = self.instance.read(cx);
        let Some(server) = entry.configuration.server.clone() else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let id = entry.id;
        let backend_handle = self.data.backend_handle.clone();

        let details = match &server.build {
            Some(build) => format!(
                "{} {} · {} {}",
                server.platform.pretty_name(),
                entry.configuration.minecraft_version,
                build_label(server.platform),
                build
            ),
            None => format!("{} {}", server.platform.pretty_name(), entry.configuration.minecraft_version),
        };

        let eula_banner = (!server.eula_accepted).then(|| {
            h_flex()
                .mx_4()
                .mb_3()
                .gap_3()
                .px_3()
                .py_2()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.warning.opacity(0.5))
                .bg(theme.warning.opacity(0.08))
                .child(Icon::new(PandoraIcon::TriangleAlert).size_4().text_color(theme.warning))
                .child(
                    v_flex()
                        .flex_1()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(t::server::eula::needed()))
                        .child(
                            div()
                                .id("eula-link")
                                .text_xs()
                                .text_color(theme.link)
                                .cursor_pointer()
                                .hover(|this| this.underline())
                                .child(t::server::eula::read())
                                .on_click(|_, _, cx| cx.open_url(crate::modals::create_server::EULA_URL)),
                        ),
                )
                .child(Button::new("accept-eula").small().warning().label(t::server::eula::accept()).on_click(
                    move |_, _, _| {
                        backend_handle.send(MessageToBackend::SetServerEulaAccepted { id, accepted: true });
                    },
                ))
        });

        v_flex()
            .child(
                h_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .child(
                        div()
                            .size_10()
                            .rounded(theme.radius)
                            .bg(theme.primary.opacity(0.12))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(platform_icon(server.platform)).size_5().text_color(theme.primary)),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(div().truncate().font_semibold().child(entry.title()))
                                    .child(status_pill(entry.status, cx)),
                            )
                            .child(div().text_xs().text_color(theme.muted_foreground).child(details)),
                    ),
            )
            .children(eula_banner)
            .into_any_element()
    }
}

impl Page for ServerPage {
    fn controls(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entry = self.instance.read(cx);
        let id = entry.id;
        let status = entry.status;
        let folder = entry.dot_minecraft_folder.clone();
        let backend_handle = self.data.backend_handle.clone();

        let restart = (status == bridge::instance::InstanceStatus::Running).then(|| {
            let backend_handle = backend_handle.clone();
            Button::new("restart-server")
                .warning()
                .icon(PandoraIcon::RefreshCcw)
                .label(t::server::action::restart())
                .on_click(move |_, window, cx| {
                    let modal_action = ModalAction::default();
                    backend_handle.send(MessageToBackend::RestartServer {
                        id,
                        modal_action: modal_action.clone(),
                    });
                    crate::modals::generic::show_notification(
                        window,
                        cx,
                        t::server::action::start_error().into(),
                        modal_action,
                    );
                })
        });

        h_flex().gap_3().child(power_button(id, status, &backend_handle)).children(restart).child(
            Button::new("open-server-folder")
                .info()
                .icon(PandoraIcon::FolderOpen)
                .label(t::server::action::open_folder())
                .on_click(move |_, window, cx| crate::open_folder(&folder, window, cx)),
        )
    }

    fn scrollable(&self, _cx: &App) -> bool {
        false
    }
}

impl Render for ServerPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let platform = self.platform(cx);
        let content_type = Self::content_type(platform);

        // Tabs are laid out dynamically because Vanilla has no content tab
        let mut tabs = vec![ServerTab::Console];
        if content_type.is_some() {
            tabs.push(ServerTab::Content);
        }
        tabs.push(ServerTab::Properties);
        tabs.push(ServerTab::Settings);

        let current = self.subpage.tab();
        let selected_index = tabs.iter().position(|tab| *tab == current).unwrap_or(0);

        let tab_bar = TabBar::new("server-tabs")
            .prefix(div().w_4())
            .selected_index(selected_index)
            .underline()
            .children(tabs.iter().map(|tab| {
                let (label, icon) = match tab {
                    ServerTab::Console => (t::server::tab::console(), PandoraIcon::SquareTerminal),
                    ServerTab::Content => match content_type {
                        Some(ContentType::Plugins) => (t::server::plugins(), PandoraIcon::Puzzle),
                        _ => (t::instance::content::mods(), PandoraIcon::PackageOpen),
                    },
                    ServerTab::Properties => (t::server::tab::properties(), PandoraIcon::SlidersVertical),
                    ServerTab::Settings => (t::settings::title(), PandoraIcon::Settings),
                };
                Tab::new().label(label).prefix(Icon::new(icon).size_4().ml_2())
            }))
            .on_click(cx.listener(move |this, index: &usize, window, cx| {
                if let Some(tab) = tabs.get(*index).copied() {
                    this.switch_tab(tab, window, cx);
                }
            }));

        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .child(tab_bar)
            .child(div().flex_1().min_h_0().child(self.subpage.clone().into_any_element()))
    }
}
