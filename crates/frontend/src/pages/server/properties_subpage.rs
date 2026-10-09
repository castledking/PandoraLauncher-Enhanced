use std::sync::Arc;

use bridge::{
    handle::BackendHandle,
    instance::{InstanceID, InstanceStatus},
    message::MessageToBackend,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Icon, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    scroll::ScrollableElement,
    spinner::Spinner,
    switch::Switch,
    v_flex,
};
use schema::server::{ServerProperty, ServerPropertyKind, server_property_kind};

use crate::{entity::instance::InstanceEntry, icon::PandoraIcon, pages::servers_page::start_server};

/// Sections the editor groups keys into, so the ~60 keys of a modern `server.properties` aren't
/// one long undifferentiated list. Anything not listed lands in "Other".
const SECTIONS: &[(fn() -> &'static str, &[&str])] = &[
    (
        t::server::properties::section_general,
        &[
            "motd",
            "max-players",
            "white-list",
            "enforce-whitelist",
            "online-mode",
            "pvp",
            "hardcore",
            "difficulty",
            "gamemode",
            "force-gamemode",
            "allow-flight",
        ],
    ),
    (
        t::server::properties::section_world,
        &[
            "level-name",
            "level-seed",
            "level-type",
            "generator-settings",
            "generate-structures",
            "allow-nether",
            "spawn-monsters",
            "spawn-protection",
            "max-world-size",
            "view-distance",
            "simulation-distance",
        ],
    ),
    (
        t::server::properties::section_network,
        &[
            "server-ip",
            "server-port",
            "enable-status",
            "enable-query",
            "query.port",
            "enable-rcon",
            "rcon.port",
            "rcon.password",
            "network-compression-threshold",
            "prevent-proxy-connections",
            "rate-limit",
        ],
    ),
    (
        t::server::properties::section_resource_pack,
        &[
            "resource-pack",
            "resource-pack-sha1",
            "resource-pack-prompt",
            "resource-pack-id",
            "require-resource-pack",
        ],
    ),
];

enum Field {
    Bool(bool),
    Input(Entity<InputState>),
    Choice(SharedString, &'static [&'static str]),
}

struct Row {
    key: Arc<str>,
    original: Arc<str>,
    field: Field,
}

enum LoadState {
    Loading,
    Loaded,
}

pub struct ServerPropertiesSubpage {
    instance: Entity<InstanceEntry>,
    instance_id: InstanceID,
    backend_handle: BackendHandle,
    rows: Vec<Row>,
    state: LoadState,
    search: Entity<InputState>,
    _load_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl ServerPropertiesSubpage {
    pub fn new(
        instance: &Entity<InstanceEntry>,
        backend_handle: BackendHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t::server::properties::search()));
        let subscriptions = vec![cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify())];

        let mut this = Self {
            instance: instance.clone(),
            instance_id: instance.read(cx).id,
            backend_handle,
            rows: Vec::new(),
            state: LoadState::Loading,
            search,
            _load_task: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.reload(window, cx);
        this
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state = LoadState::Loading;
        let (send, recv) = tokio::sync::oneshot::channel();
        self.backend_handle.send(MessageToBackend::GetServerProperties {
            id: self.instance_id,
            channel: send,
        });

        self._load_task = cx.spawn_in(window, async move |this, cx| {
            let properties = recv.await.unwrap_or_default();
            _ = this.update_in(cx, |this, window, cx| {
                this.rows = properties.into_iter().map(|property| make_row(property, window, cx)).collect();
                this.state = LoadState::Loaded;
                cx.notify();
            });
        });
        cx.notify();
    }

    fn current_value(row: &Row, cx: &App) -> Arc<str> {
        match &row.field {
            Field::Bool(value) => if *value { "true" } else { "false" }.into(),
            Field::Input(input) => input.read(cx).value().as_str().into(),
            Field::Choice(value, _) => value.as_str().into(),
        }
    }

    fn changed_count(&self, cx: &App) -> usize {
        self.rows.iter().filter(|row| Self::current_value(row, cx) != row.original).count()
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let properties: Arc<[ServerProperty]> = self
            .rows
            .iter()
            .map(|row| ServerProperty {
                key: row.key.clone(),
                value: Self::current_value(row, cx),
            })
            .collect();

        for row in &mut self.rows {
            row.original = Self::current_value(row, cx);
        }

        self.backend_handle.send(MessageToBackend::SetServerProperties {
            id: self.instance_id,
            properties,
        });
        cx.notify();
    }

    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for row in &mut self.rows {
            match &mut row.field {
                Field::Bool(value) => *value = &*row.original == "true",
                Field::Input(input) => {
                    let original = row.original.clone();
                    input.update(cx, |input, cx| input.set_value(original.to_string(), window, cx));
                },
                Field::Choice(value, _) => *value = row.original.to_string().into(),
            }
        }
        cx.notify();
    }

    fn render_row(&self, index: usize, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let changed = Self::current_value(row, cx) != row.original;

        let widget = match &row.field {
            Field::Bool(value) => Switch::new(("prop-switch", index))
                .checked(*value)
                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                    if let Some(Row {
                        field: Field::Bool(value),
                        ..
                    }) = this.rows.get_mut(index)
                    {
                        *value = *checked;
                        cx.notify();
                    }
                }))
                .into_any_element(),
            Field::Input(input) => Input::new(input).small().into_any_element(),
            Field::Choice(value, choices) => {
                let choices = *choices;
                Button::new(("prop-choice", index))
                    .small()
                    .outline()
                    .label(value.clone())
                    .icon(PandoraIcon::ChevronDown)
                    .dropdown_menu({
                        let entity = cx.entity();
                        move |mut menu, _, _| {
                            for choice in choices {
                                let entity = entity.clone();
                                menu = menu.item(PopupMenuItem::new(*choice).on_click(move |_, _, cx| {
                                    entity.update(cx, |this, cx| {
                                        if let Some(Row {
                                            field: Field::Choice(value, _),
                                            ..
                                        }) = this.rows.get_mut(index)
                                        {
                                            *value = (*choice).into();
                                            cx.notify();
                                        }
                                    });
                                }));
                            }
                            menu
                        }
                    })
                    .into_any_element()
            },
        };

        h_flex()
            .w_full()
            .gap_4()
            .px_3()
            .py_2()
            .rounded(theme.radius)
            .when(changed, |this| this.bg(theme.primary.opacity(0.06)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(div().text_sm().font_medium().child(pretty_key(&row.key)))
                            .when(changed, |this| this.child(div().size_1p5().rounded_full().bg(theme.primary))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family("Roboto Mono")
                            .text_color(theme.muted_foreground)
                            .child(row.key.to_string()),
                    ),
            )
            .child(h_flex().w(px(240.0)).flex_shrink_0().justify_end().child(widget))
            .into_any_element()
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let id = self.instance_id;
        let backend_handle = self.backend_handle.clone();
        let offline = self.instance.read(cx).status == InstanceStatus::NotRunning;

        div().size_full().flex().items_center().justify_center().child(
            v_flex()
                .items_center()
                .gap_3()
                .max_w(px(380.0))
                .child(Icon::new(PandoraIcon::SlidersVertical).size_8().text_color(theme.muted_foreground))
                .child(div().font_semibold().child(t::server::properties::empty_title()))
                .child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(theme.muted_foreground)
                        .child(t::server::properties::empty_body()),
                )
                .when(offline, |this| {
                    this.child(
                        Button::new("properties-start")
                            .success()
                            .icon(PandoraIcon::Play)
                            .label(t::server::action::start())
                            .on_click(move |_, window, cx| start_server(id, &backend_handle, window, cx)),
                    )
                }),
        )
    }
}

impl Render for ServerPropertiesSubpage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let LoadState::Loading = self.state {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element();
        }
        if self.rows.is_empty() {
            return self.render_empty(cx).into_any_element();
        }

        let theme = cx.theme().clone();
        let query = self.search.read(cx).value().to_lowercase();
        let changed = self.changed_count(cx);
        let running = self.instance.read(cx).status == InstanceStatus::Running;

        let matches = |row: &Row| {
            query.is_empty()
                || row.key.to_lowercase().contains(&query)
                || pretty_key(&row.key).to_lowercase().contains(&query)
        };

        let mut sections: Vec<(&'static str, Vec<usize>)> =
            SECTIONS.iter().map(|(title, _)| (title(), Vec::new())).collect();
        let mut other = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            if !matches(row) {
                continue;
            }
            match SECTIONS.iter().position(|(_, keys)| keys.contains(&&*row.key)) {
                Some(section) => sections[section].1.push(index),
                None => other.push(index),
            }
        }
        // Keep each section in the order the keys are listed above, not file order
        for (section, (_, keys)) in sections.iter_mut().zip(SECTIONS) {
            section.1.sort_by_key(|index| keys.iter().position(|key| *key == &*self.rows[*index].key));
        }
        other.sort_by(|a, b| self.rows[*a].key.cmp(&self.rows[*b].key));
        sections.push((t::server::properties::section_other(), other));

        let mut body = v_flex().gap_5().p_4();
        let mut any = false;
        for (title, indices) in sections {
            if indices.is_empty() {
                continue;
            }
            any = true;
            body = body.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .px_3()
                            .text_xs()
                            .font_semibold()
                            .text_color(theme.muted_foreground)
                            .child(title.to_uppercase()),
                    )
                    .child(v_flex().p_1().rounded(theme.radius_lg).border_1().border_color(theme.border).children({
                        let mut rows = Vec::with_capacity(indices.len());
                        for index in indices {
                            rows.push(self.render_row(index, &self.rows[index], cx));
                        }
                        rows
                    })),
            );
        }
        if !any {
            body = body.child(
                div()
                    .p_4()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(t::server::properties::no_match()),
            );
        }

        let toolbar = h_flex()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .w(px(280.0))
                    .child(Input::new(&self.search).small().prefix(Icon::new(PandoraIcon::Search).size_4())),
            )
            .child(div().flex_1())
            .when(running && changed > 0, |this| {
                this.child(div().text_xs().text_color(theme.warning).child(t::server::properties::restart_needed()))
            })
            .child(
                Button::new("reload-properties")
                    .small()
                    .ghost()
                    .icon(PandoraIcon::RefreshCcw)
                    .tooltip(t::server::properties::reload())
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            )
            .child(
                Button::new("revert-properties")
                    .small()
                    .outline()
                    .label(t::server::properties::revert())
                    .disabled(changed == 0)
                    .on_click(cx.listener(|this, _, window, cx| this.revert(window, cx))),
            )
            .child(
                Button::new("save-properties")
                    .small()
                    .success()
                    .label(if changed == 0 {
                        t::server::properties::saved().to_string()
                    } else {
                        t::server::properties::save(changed)
                    })
                    .disabled(changed == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            );

        v_flex()
            .size_full()
            .child(toolbar)
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(body))
            .into_any_element()
    }
}

fn make_row(property: ServerProperty, window: &mut Window, cx: &mut Context<ServerPropertiesSubpage>) -> Row {
    let field = match server_property_kind(&property.key) {
        ServerPropertyKind::Bool => Field::Bool(&*property.value == "true"),
        ServerPropertyKind::Choice(choices) => Field::Choice(property.value.to_string().into(), choices),
        ServerPropertyKind::Integer { .. } | ServerPropertyKind::Text => {
            let value = property.value.to_string();
            let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
            Field::Input(input)
        },
    };
    Row {
        key: property.key.clone(),
        original: property.value,
        field,
    }
}

/// `max-players` → "Max players", `rcon.port` → "RCON port", `pvp` → "PvP".
fn pretty_key(key: &str) -> String {
    let words =
        key.split(['-', '.', '_'])
            .filter(|word| !word.is_empty())
            .enumerate()
            .map(|(index, word)| match word {
                "pvp" => "PvP".to_string(),
                "motd" => "MOTD".to_string(),
                "ip" => "IP".to_string(),
                "id" => "ID".to_string(),
                "rcon" => "RCON".to_string(),
                "jmx" => "JMX".to_string(),
                "sha1" => "SHA-1".to_string(),
                _ if index == 0 => {
                    let mut chars = word.chars();
                    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
                },
                _ => word.to_string(),
            });
    words.collect::<Vec<_>>().join(" ")
}
