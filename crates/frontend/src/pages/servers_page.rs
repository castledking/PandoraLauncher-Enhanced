use bridge::{
    handle::BackendHandle,
    instance::{InstanceID, InstanceStatus},
    message::MessageToBackend,
    modal_action::ModalAction,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, IndexPath, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    menu::{DropdownMenu, PopupMenuItem},
    select::{Select, SelectEvent, SelectState},
    table::{Column, ColumnSort, DataTable, TableDelegate, TableState},
    v_flex,
};
use schema::server::ServerPlatform;
use strum::IntoEnumIterator;

use crate::{
    component::{
        named_dropdown::{NamedDropdown, NamedDropdownItem},
        responsive_grid::ResponsiveGrid,
    },
    entity::{
        DataEntities,
        instance::{InstanceAddedEvent, InstanceEntries, InstanceEntry, InstanceModifiedEvent, InstanceRemovedEvent},
    },
    icon::PandoraIcon,
    interface_config::{InterfaceConfig, ViewMode},
    pages::page::Page,
    png_render_cache, root, ui,
};

/// The servers among the instances, in the order the page shows them.
fn servers_of(entries: &InstanceEntries, cx: &App) -> Vec<InstanceEntry> {
    let mut servers: Vec<InstanceEntry> = entries
        .entries
        .values()
        .map(|entry| entry.read(cx).clone())
        .filter(|entry| entry.configuration.server.is_some())
        .collect();
    sort_servers(&mut servers);
    servers
}

/// Running servers first, since those are the ones being worked on.
fn sort_servers(servers: &mut [InstanceEntry]) {
    servers.sort_by(|a, b| {
        let running = |entry: &InstanceEntry| entry.status != InstanceStatus::NotRunning;
        running(b)
            .cmp(&running(a))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// A server that somehow lost its configuration still needs a row, and one with no platform
/// to sort by is better sorted as if it were the plainest option than not sorted at all.
fn platform_of(server: &InstanceEntry) -> ServerPlatform {
    server.configuration.server.as_ref().map_or(ServerPlatform::Vanilla, |c| c.platform)
}

pub struct ServersPage {
    data: DataEntities,
    server_table: Entity<TableState<ServerList>>,
    view_dropdown: Entity<SelectState<NamedDropdown<ViewMode>>>,
    _subscriptions: [Subscription; 3],
}

impl ServersPage {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = [
            cx.subscribe::<_, InstanceAddedEvent>(&data.instances, |_, _, _, cx| cx.notify()),
            cx.subscribe::<_, InstanceRemovedEvent>(&data.instances, |_, _, _, cx| cx.notify()),
            cx.subscribe::<_, InstanceModifiedEvent>(&data.instances, |_, _, _, cx| cx.notify()),
        ];

        let server_table = ServerList::create_table(data, window, cx);
        let view_dropdown = cx.new(|cx| {
            let items = ViewMode::iter()
                .map(|view| NamedDropdownItem {
                    name: view.name(),
                    item: view,
                })
                .collect::<Vec<_>>();
            let current_view = InterfaceConfig::get(cx).servers_view_mode;
            let row = items.iter().position(|v| v.item == current_view).unwrap_or(0);
            SelectState::new(NamedDropdown::new(items), Some(IndexPath::new(row)), window, cx)
        });
        cx.subscribe(&view_dropdown, |_, _, event: &SelectEvent<NamedDropdown<ViewMode>>, cx| {
            let SelectEvent::Confirm(Some(view)) = event else {
                return;
            };
            InterfaceConfig::get_mut(cx).servers_view_mode = *view;
        })
        .detach();

        Self {
            data: data.clone(),
            server_table,
            view_dropdown,
            _subscriptions: subscriptions,
        }
    }

    fn open_create(&self, window: &mut Window, cx: &mut App) {
        crate::modals::create_server::open_create_server(
            self.data.metadata.clone(),
            self.data.instances.clone(),
            self.data.backend_handle.clone(),
            None,
            window,
            cx,
        );
    }
}

impl Page for ServersPage {
    fn controls(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let create_server = Button::new("create_server")
            .success()
            .icon(PandoraIcon::Plus)
            .label(t::server::create::title())
            .on_click(cx.listener(|this, _, window, cx| this.open_create(window, cx)));

        // wrapping in div makes it not take up the full space of the titlebar
        let select_view =
            div().child(Select::new(&self.view_dropdown).title_prefix(format!("{}: ", t::instance::view_mode())));

        h_flex().gap_3().child(create_server).child(select_view)
    }

    fn scrollable(&self, cx: &App) -> bool {
        match InterfaceConfig::get(cx).servers_view_mode {
            ViewMode::Cards => true,
            ViewMode::List => false,
        }
    }
}

impl Render for ServersPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let servers = servers_of(&self.data.instances.read(cx), cx);

        if servers.is_empty() {
            return render_empty_state(cx).into_any_element();
        }

        match InterfaceConfig::get(cx).servers_view_mode {
            ViewMode::Cards => {
                let running = servers.iter().filter(|server| server.status == InstanceStatus::Running).count();
                let header = h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t::server::list::summary(servers.len(), running));

                let cards = servers
                    .iter()
                    .map(|server| render_server_card(server, &self.data.backend_handle, cx))
                    .collect::<Vec<_>>();

                let size = Size::new(AvailableSpace::MinContent, AvailableSpace::MinContent);

                v_flex()
                    .size_full()
                    .p_4()
                    .gap_3()
                    .child(header)
                    .child(ResponsiveGrid::new(size).size_full().gap_4().children(cards))
                    .into_any_element()
            },
            // The table brings its own scrolling and its own header, so the card summary
            // above would be sitting on top of the columns
            ViewMode::List => DataTable::new(&self.server_table).bordered(false).into_any_element(),
        }
    }
}

/// The row's overflow menu, shared by the card and the table row so both offer the same
/// actions on the same terms.
fn server_menu(server: &InstanceEntry, backend_handle: &BackendHandle, index: usize) -> impl IntoElement {
    let id = server.id;
    let name = server.name.clone();
    let folder = server.dot_minecraft_folder.clone();
    let backend_handle = backend_handle.clone();

    Button::new(("server-menu", index))
        .ghost()
        .small()
        .icon(PandoraIcon::EllipsisVertical)
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
            menu.item(PopupMenuItem::new(t::server::action::open_folder()).on_click({
                let folder = folder.clone();
                move |_, window, cx| crate::open_folder(&folder, window, cx)
            }))
            .separator()
            .item(
                crate::component::instance_list::delete_menu_item(t::server::action::delete()).on_click({
                    let name = name.clone();
                    let backend_handle = backend_handle.clone();
                    move |_, window, cx| {
                        crate::component::instance_list::delete_instance(
                            id,
                            &name,
                            &backend_handle,
                            window.modifiers().shift,
                            window,
                            cx,
                        );
                    }
                }),
            )
        })
}

fn render_empty_state(cx: &mut Context<ServersPage>) -> impl IntoElement {
    let theme = cx.theme();

    let feature = |icon: PandoraIcon, title: &'static str, description: &'static str| {
        h_flex()
            .gap_3()
            .items_start()
            .child(
                div()
                    .flex_shrink_0()
                    .size_8()
                    .rounded(theme.radius)
                    .bg(theme.primary.opacity(0.12))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(icon).size_4().text_color(theme.primary)),
            )
            .child(
                v_flex()
                    .child(div().text_sm().font_medium().child(title))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(description)),
            )
    };

    div().size_full().flex().items_center().justify_center().p_8().child(
        v_flex()
            .max_w(px(460.0))
            .gap_6()
            .items_center()
            .child(
                div()
                    .size_16()
                    .rounded_xl()
                    .bg(theme.primary.opacity(0.12))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(PandoraIcon::Server).size_8().text_color(theme.primary)),
            )
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .child(div().text_xl().font_semibold().child(t::server::list::empty_title()))
                    .child(
                        div()
                            .text_sm()
                            .text_center()
                            .text_color(theme.muted_foreground)
                            .child(t::server::list::empty_body()),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap_3()
                    .p_4()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .child(feature(
                        PandoraIcon::SquareTerminal,
                        t::server::list::feature_console(),
                        t::server::list::feature_console_desc(),
                    ))
                    .child(feature(
                        PandoraIcon::Puzzle,
                        t::server::list::feature_content(),
                        t::server::list::feature_content_desc(),
                    ))
                    .child(feature(
                        PandoraIcon::RefreshCcw,
                        t::server::list::feature_sync(),
                        t::server::list::feature_sync_desc(),
                    )),
            )
            .child(
                Button::new("create_first_server")
                    .success()
                    .large()
                    .icon(PandoraIcon::Plus)
                    .label(t::server::list::create_first())
                    .on_click(cx.listener(|this, _, window, cx| this.open_create(window, cx))),
            ),
    )
}

pub fn platform_icon(platform: ServerPlatform) -> PandoraIcon {
    match platform {
        ServerPlatform::Vanilla => PandoraIcon::Box,
        ServerPlatform::Paper => PandoraIcon::Feather,
        ServerPlatform::Purpur => PandoraIcon::Zap,
        ServerPlatform::Fabric => PandoraIcon::Layers,
        ServerPlatform::Forge => PandoraIcon::Anvil,
        ServerPlatform::NeoForge => PandoraIcon::Swords,
    }
}

/// Paper and Purpur number their builds; for the modded platforms the resolved value is the
/// loader version, so it's labelled as such.
pub fn build_label(platform: ServerPlatform) -> &'static str {
    if platform.needs_build() {
        t::server::build()
    } else {
        t::server::loader()
    }
}

/// The coloured dot and label used everywhere a server's state is shown.
pub fn status_pill(status: InstanceStatus, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (label, color) = match status {
        InstanceStatus::Running => (t::server::status::online(), theme.success),
        InstanceStatus::Launching => (t::server::status::starting(), theme.warning),
        InstanceStatus::Stopping => (t::server::status::stopping(), theme.warning),
        InstanceStatus::NotRunning => (t::server::status::offline(), theme.muted_foreground),
    };

    h_flex()
        .flex_shrink_0()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(color.opacity(0.12))
        .text_xs()
        .font_medium()
        .text_color(color)
        .child(div().size_1p5().rounded_full().bg(color))
        .child(label)
}

/// Start, or stop when it's running. Shared by the cards and the server page.
pub fn power_button(id: InstanceID, status: InstanceStatus, backend_handle: &BackendHandle) -> Button {
    let backend_handle = backend_handle.clone();
    match status {
        InstanceStatus::NotRunning => Button::new(("server-start", id.index))
            .success()
            .icon(PandoraIcon::Play)
            .label(t::server::action::start())
            .on_click(move |_, window, cx| start_server(id, &backend_handle, window, cx)),
        InstanceStatus::Running => Button::new(("server-stop", id.index))
            .danger()
            .icon(PandoraIcon::Close)
            .label(t::server::action::stop())
            .on_click(move |_, _, _| backend_handle.send(MessageToBackend::StopServer { id })),
        InstanceStatus::Launching => Button::new(("server-starting", id.index))
            .warning()
            .icon(PandoraIcon::Loader)
            .label(t::server::status::starting()),
        // Stopping has hung if it's still pressed, so offer the hard kill
        InstanceStatus::Stopping => Button::new(("server-kill", id.index))
            .danger()
            .icon(PandoraIcon::Loader)
            .label(t::server::action::force_stop())
            .on_click(move |_, _, _| backend_handle.send(MessageToBackend::KillInstance { id })),
    }
}

pub fn start_server(id: InstanceID, backend_handle: &BackendHandle, window: &mut Window, cx: &mut App) {
    let modal_action = ModalAction::default();
    backend_handle.send(MessageToBackend::StartServer {
        id,
        modal_action: modal_action.clone(),
    });
    crate::modals::generic::show_notification(window, cx, t::server::action::start_error().into(), modal_action);
}

fn render_server_card(server: &InstanceEntry, backend_handle: &BackendHandle, cx: &mut App) -> Div {
    let Some(config) = &server.configuration.server else {
        return div();
    };
    let theme = cx.theme().clone();
    let id = server.id;
    let index = id.index;

    let icon = if let Some(icon) = server.icon.clone() {
        let transform = png_render_cache::ImageTransformation::Resize { width: 48, height: 48 };
        png_render_cache::render_with_transform(icon, transform, cx)
            .rounded(theme.radius)
            .size_12()
            .into_any_element()
    } else {
        div()
            .size_12()
            .rounded(theme.radius)
            .bg(theme.primary.opacity(0.12))
            .flex()
            .items_center()
            .justify_center()
            .child(Icon::new(platform_icon(config.platform)).size_6().text_color(theme.primary))
            .into_any_element()
    };

    let subtitle = match &config.build {
        Some(build) => {
            format!(
                "{} {} · {} {}",
                config.platform.pretty_name(),
                server.configuration.minecraft_version,
                build_label(config.platform),
                build
            )
        },
        None => format!("{} {}", config.platform.pretty_name(), server.configuration.minecraft_version),
    };

    let menu = server_menu(server, backend_handle, index);

    let warning = (!config.eula_accepted).then(|| {
        h_flex()
            .gap_1p5()
            .text_xs()
            .text_color(theme.warning)
            .child(Icon::new(PandoraIcon::TriangleAlert).size_3p5())
            .child(t::server::eula::needed_short())
    });

    let linked = config.linked_instance.clone().map(|linked| {
        h_flex()
            .gap_1p5()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(Icon::new(PandoraIcon::Link).size_3p5())
            .child(div().truncate().child(t::server::list::linked_to(&linked)))
    });

    v_flex()
        .relative()
        .flex_1()
        .min_w_72()
        .p_3()
        .gap_3()
        .border_1()
        .border_color(if server.status == InstanceStatus::Running {
            theme.success.opacity(0.5)
        } else {
            theme.border
        })
        .rounded(theme.radius_lg)
        .child(div().absolute().right_2().top_2().child(menu))
        .child(
            h_flex().gap_3().pr_8().child(icon).child(
                v_flex()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().truncate().font_semibold().child(server.name.clone()))
                    .child(div().truncate().text_xs().text_color(theme.muted_foreground).child(subtitle)),
            ),
        )
        .child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(status_pill(server.status, cx))
                .children(warning)
                .children(linked),
        )
        .child(
            h_flex()
                .gap_2()
                .child(power_button(id, server.status, backend_handle).flex_1().small())
                .child(
                    Button::new(("server-open", index))
                        .flex_1()
                        .small()
                        .info()
                        .icon(PandoraIcon::Settings2)
                        .label(t::server::action::manage())
                        .on_click({
                            let name = server.name.clone();
                            move |_, window, cx| {
                                root::switch_page(
                                    ui::PageType::ServerPage { name: name.clone() },
                                    &[ui::PageType::Servers],
                                    window,
                                    cx,
                                );
                            }
                        }),
                ),
        )
}

/// The list view's backing data, kept in step with the instance entries the same way the
/// instance list is, so a server starting, renaming or going away shows up without the
/// page having to rebuild the table.
pub struct ServerList {
    columns: Vec<Column>,
    items: Vec<InstanceEntry>,
    data: DataEntities,
    _instance_added_subscription: Subscription,
    _instance_removed_subscription: Subscription,
    _instance_modified_subscription: Subscription,
}

impl ServerList {
    fn create_table(data: &DataEntities, window: &mut Window, cx: &mut App) -> Entity<TableState<Self>> {
        let instances = data.instances.clone();
        let items = servers_of(&instances.read(cx), cx);

        cx.new(|cx| {
            let _instance_added_subscription =
                cx.subscribe::<_, InstanceAddedEvent>(&instances, |table: &mut TableState<Self>, _, event, cx| {
                    if event.instance.configuration.server.is_none() {
                        return;
                    }
                    let items = &mut table.delegate_mut().items;
                    items.retain(|entry| entry.id != event.instance.id);
                    items.insert(0, event.instance.clone());
                    sort_servers(items);
                    cx.notify();
                });
            let _instance_removed_subscription =
                cx.subscribe::<_, InstanceRemovedEvent>(&instances, |table, _, event, cx| {
                    table.delegate_mut().items.retain(|entry| entry.id != event.id);
                    cx.notify();
                });
            let _instance_modified_subscription =
                cx.subscribe::<_, InstanceModifiedEvent>(&instances, |table, _, event, cx| {
                    if event.instance.configuration.server.is_some()
                        && let Some(entry) =
                            table.delegate_mut().items.iter_mut().find(|entry| entry.id == event.instance.id)
                    {
                        *entry = event.instance.clone();
                        cx.notify();
                    }
                });

            let server_list = Self {
                columns: vec![
                    Column::new("controls", "").width(150.).fixed_left().movable(false).resizable(false),
                    Column::new("name", t::instance::name())
                        .width(200.)
                        .fixed_left()
                        .sortable()
                        .resizable(true),
                    Column::new("platform", t::server::column::platform())
                        .width(120.)
                        .fixed_left()
                        .sortable()
                        .resizable(true),
                    Column::new("version", t::server::column::version())
                        .width(120.)
                        .fixed_left()
                        .sortable()
                        .resizable(true),
                    Column::new("status", t::server::column::status())
                        .width(120.)
                        .fixed_left()
                        .resizable(true),
                    Column::new("menu", "").width(44.).fixed_left().movable(false).resizable(false),
                ],
                items,
                data: data.clone(),
                _instance_added_subscription,
                _instance_removed_subscription,
                _instance_modified_subscription,
            };
            TableState::new(server_list, window, cx)
        })
    }
}

impl TableDelegate for ServerList {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.items.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) {
        let Some(col) = self.columns.get_mut(col_ix) else {
            return;
        };

        // The running-first ordering is the page's own, so it's kept as the starting point
        // and only replaced once a column has actually been chosen
        match col.key.as_ref() {
            "name" => self.items.sort_by(|a, b| {
                let ordering = lexical_sort::natural_lexical_cmp(&a.name, &b.name);
                if sort == ColumnSort::Descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            }),
            "platform" => self.items.sort_by(|a, b| {
                let ordering = platform_of(a).pretty_name().cmp(platform_of(b).pretty_name());
                if sort == ColumnSort::Descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            }),
            "version" => self.items.sort_by(|a, b| {
                let ordering = lexical_sort::natural_lexical_cmp(
                    &a.configuration.minecraft_version,
                    &b.configuration.minecraft_version,
                );
                if sort == ColumnSort::Descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            }),
            _ => {},
        }
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(server) = self.items.get(row_ix) else {
            return div().into_any_element();
        };
        let Some(config) = server.configuration.server.clone() else {
            return div().into_any_element();
        };
        let backend_handle = self.data.backend_handle.clone();
        let id = server.id;
        let index = id.index;
        let status = server.status;

        let Some(col) = self.columns.get(col_ix).cloned() else {
            return div().into_any_element();
        };

        match col.key.as_ref() {
            "name" => h_flex()
                .gap_2()
                .items_center()
                .child(div().size_6().flex_shrink_0().child(platform_icon(config.platform)))
                .child(div().truncate().child(server.name.clone()))
                .into_any_element(),
            "platform" => config.platform.pretty_name().into_any_element(),
            "version" => server.configuration.minecraft_version.as_str().into_any_element(),
            "status" => status_pill(status, cx).into_any_element(),
            "controls" => h_flex()
                .size_full()
                .gap_2()
                .child(power_button(id, status, &backend_handle).w_1_2().small())
                .child(Button::new("view").w_1_2().small().info().label(t::server::action::manage()).on_click({
                    let name = server.name.clone();
                    move |_, window, cx| {
                        root::switch_page(
                            ui::PageType::ServerPage { name: name.clone() },
                            &[ui::PageType::Servers],
                            window,
                            cx,
                        );
                    }
                }))
                .into_any_element(),
            "menu" => server_menu(server, &backend_handle, index).into_any_element(),
            _ => div().into_any_element(),
        }
    }
}
