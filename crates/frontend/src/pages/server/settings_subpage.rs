use std::{path::Path, sync::Arc};

use bridge::{handle::BackendHandle, instance::InstanceID, message::MessageToBackend};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IndexPath, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState, NumberInput, NumberInputEvent, StepAction},
    scroll::ScrollableElement,
    select::{Select, SelectEvent, SelectState},
    switch::Switch,
    v_flex,
};
use schema::{
    instance::{InstanceJvmBinaryConfiguration, InstanceJvmFlagsConfiguration, InstanceMemoryConfiguration},
    loader::Loader,
};

use crate::{
    component::named_dropdown::{DropdownName, NamedDropdown, NamedDropdownItem},
    entity::{DataEntities, instance::InstanceEntry},
    icon::PandoraIcon,
};

/// Servers need more than a client by default, so the fields start somewhere useful rather than
/// at the client defaults.
const DEFAULT_MIN_MEMORY: u32 = 1024;
const DEFAULT_MAX_MEMORY: u32 = 4096;

pub struct ServerSettingsSubpage {
    instance: Entity<InstanceEntry>,
    instance_id: InstanceID,
    backend_handle: BackendHandle,
    memory_enabled: bool,
    memory_min: Entity<InputState>,
    memory_max: Entity<InputState>,
    jvm_flags_enabled: bool,
    jvm_flags: Entity<InputState>,
    java_enabled: bool,
    java_path: Option<Arc<Path>>,
    sync_source: Entity<SelectState<NamedDropdown<Option<InstanceID>>>>,
    _select_java_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl ServerSettingsSubpage {
    pub fn new(
        instance: &Entity<InstanceEntry>,
        data: &DataEntities,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let entry = instance.read(cx);
        let instance_id = entry.id;
        let configuration = entry.configuration.clone();

        // Unset memory shows as off, with sensible numbers ready for when it's switched on, so the
        // page never claims a limit the server isn't actually running with
        let memory = configuration.memory.unwrap_or(InstanceMemoryConfiguration {
            enabled: false,
            min: DEFAULT_MIN_MEMORY,
            max: DEFAULT_MAX_MEMORY,
        });
        let memory_min = cx.new(|cx| InputState::new(window, cx).default_value(memory.min.to_string()));
        let memory_max = cx.new(|cx| InputState::new(window, cx).default_value(memory.max.to_string()));

        let jvm_flags = configuration.jvm_flags.clone().unwrap_or_default();
        let jvm_flags_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t::server::settings::jvm_flags_placeholder())
                .default_value(jvm_flags.flags.to_string())
        });

        let jvm_binary = configuration.jvm_binary.clone().unwrap_or_default();

        // Client instances on the same loader are the only sensible things to sync mods from
        let linked = configuration.server.as_ref().and_then(|server| server.linked_instance.clone());
        let server_loader = configuration.loader;
        let mut sources = vec![NamedDropdownItem {
            name: DropdownName::translated(t::server::settings::sync_pick),
            item: None,
        }];
        let mut preselected = 0;
        for other in data.instances.read(cx).entries.values() {
            let other = other.read(cx);
            if other.configuration.server.is_some()
                || other.name == schema::quickplay::INSTANCE_NAME
                || other.configuration.loader == Loader::Vanilla
                || other.configuration.loader != server_loader
            {
                continue;
            }
            if linked.as_deref() == Some(other.name.as_str()) {
                preselected = sources.len();
            }
            sources.push(NamedDropdownItem {
                name: DropdownName::new(format!(
                    "{} ({} {})",
                    other.name,
                    other.configuration.loader.pretty_name(),
                    other.configuration.minecraft_version
                )),
                item: Some(other.id),
            });
        }
        let sync_source =
            cx.new(|cx| SelectState::new(NamedDropdown::new(sources), Some(IndexPath::new(preselected)), window, cx));

        let subscriptions = vec![
            cx.subscribe_in(&memory_min, window, Self::on_memory_step),
            cx.subscribe_in(&memory_max, window, Self::on_memory_step),
            cx.subscribe(&memory_min, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.send_memory(cx);
                }
            }),
            cx.subscribe(&memory_max, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.send_memory(cx);
                }
            }),
            cx.subscribe(&jvm_flags_input, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.send_jvm_flags(cx);
                }
            }),
            cx.subscribe(&sync_source, |_, _, _: &SelectEvent<NamedDropdown<Option<InstanceID>>>, cx| {
                cx.notify();
            }),
            cx.observe(instance, |_, _, cx| cx.notify()),
        ];

        Self {
            instance: instance.clone(),
            instance_id,
            backend_handle: data.backend_handle.clone(),
            memory_enabled: memory.enabled,
            memory_min,
            memory_max,
            jvm_flags_enabled: jvm_flags.enabled,
            jvm_flags: jvm_flags_input,
            java_enabled: jvm_binary.enabled,
            java_path: jvm_binary.path.clone(),
            sync_source,
            _select_java_task: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    fn on_memory_step(
        &mut self,
        state: &Entity<InputState>,
        event: &NumberInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let NumberInputEvent::Step(action) = event;
        let Ok(value) = state.read(cx).value().parse::<u32>() else {
            return;
        };
        // Steps in 512 MiB, which is the granularity people actually think about server RAM in
        let value = match action {
            StepAction::Increment => (value / 512 + 1) * 512,
            StepAction::Decrement => (value.saturating_sub(1) / 512).saturating_mul(512).max(512),
        };
        state.update(cx, |input, cx| input.set_value(value.to_string(), window, cx));
    }

    fn send_memory(&self, cx: &App) {
        let min = self.memory_min.read(cx).value().parse::<u32>().unwrap_or(DEFAULT_MIN_MEMORY);
        let max = self.memory_max.read(cx).value().parse::<u32>().unwrap_or(DEFAULT_MAX_MEMORY);
        self.backend_handle.send(MessageToBackend::SetInstanceMemory {
            id: self.instance_id,
            memory: InstanceMemoryConfiguration {
                enabled: self.memory_enabled,
                min,
                max,
            },
        });
    }

    fn send_jvm_flags(&self, cx: &App) {
        self.backend_handle.send(MessageToBackend::SetInstanceJvmFlags {
            id: self.instance_id,
            jvm_flags: InstanceJvmFlagsConfiguration {
                enabled: self.jvm_flags_enabled,
                flags: self.jvm_flags.read(cx).value().as_str().into(),
            },
        });
    }

    fn send_java(&self) {
        self.backend_handle.send(MessageToBackend::SetInstanceJvmBinary {
            id: self.instance_id,
            jvm_binary: InstanceJvmBinaryConfiguration {
                enabled: self.java_enabled,
                path: self.java_path.clone(),
            },
        });
    }

    fn pick_java(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some(t::server::settings::java_pick().into()),
        });
        self._select_java_task = cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = receiver.await else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            let path: Arc<Path> = paths.swap_remove(0).into();
            _ = this.update(cx, |this, cx| {
                this.java_path = Some(path);
                this.java_enabled = true;
                this.send_java();
                cx.notify();
            });
        });
    }
}

/// A titled, bordered block, so the settings read as a few distinct groups rather than a wall of
/// fields.
fn card(icon: PandoraIcon, title: &'static str, description: &'static str, cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .gap_3()
        .p_4()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .gap_3()
                .child(
                    div()
                        .size_8()
                        .flex_shrink_0()
                        .rounded(theme.radius)
                        .bg(theme.primary.opacity(0.12))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(icon).size_4().text_color(theme.primary)),
                )
                .child(
                    v_flex()
                        .child(div().font_semibold().child(title))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(description)),
                ),
        )
}

impl Render for ServerSettingsSubpage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entry = self.instance.read(cx);
        let Some(server) = entry.configuration.server.clone() else {
            return div().into_any_element();
        };
        let id = self.instance_id;
        let is_running = entry.status != bridge::instance::InstanceStatus::NotRunning;
        let server_name = entry.name.clone();
        let theme = cx.theme().clone();

        let eula = card(
            PandoraIcon::BookOpen,
            t::server::settings::eula_title(),
            t::server::settings::eula_desc(),
            cx,
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Checkbox::new("eula")
                        .label(t::server::eula::agree())
                        .checked(server.eula_accepted)
                        .on_click(cx.listener(move |this, value: &bool, _, _| {
                            this.backend_handle.send(MessageToBackend::SetServerEulaAccepted { id, accepted: *value });
                        })),
                )
                .child(div().flex_1())
                .child(
                    Button::new("read-eula")
                        .xsmall()
                        .ghost()
                        .icon(PandoraIcon::ExternalLink)
                        .label(t::server::eula::read())
                        .on_click(|_, _, cx| cx.open_url(crate::modals::create_server::EULA_URL)),
                ),
        );

        let memory_enabled = self.memory_enabled;
        let resources = card(
            PandoraIcon::Cpu,
            t::server::settings::memory_title(),
            t::server::settings::memory_desc(),
            cx,
        )
        .child(
            Checkbox::new("memory-enabled")
                .label(t::server::settings::memory_custom())
                .checked(memory_enabled)
                .on_click(cx.listener(|this, value: &bool, _, cx| {
                    this.memory_enabled = *value;
                    this.send_memory(cx);
                    cx.notify();
                })),
        )
        .child(
            h_flex()
                .gap_3()
                .child(
                    crate::labelled(
                        t::common::min(),
                        NumberInput::new(&self.memory_min)
                            .small()
                            .suffix(t::common::size::mib())
                            .disabled(!memory_enabled),
                    )
                    .flex_1(),
                )
                .child(
                    crate::labelled(
                        t::common::max(),
                        NumberInput::new(&self.memory_max)
                            .small()
                            .suffix(t::common::size::mib())
                            .disabled(!memory_enabled),
                    )
                    .flex_1(),
                ),
        );

        let jvm_flags_enabled = self.jvm_flags_enabled;
        let java_enabled = self.java_enabled;
        let java_label: SharedString = match &self.java_path {
            Some(path) => path.to_string_lossy().into_owned().into(),
            None => t::server::settings::java_none().into(),
        };
        let java = card(
            PandoraIcon::CodeXml,
            t::server::settings::java_title(),
            t::server::settings::java_desc(),
            cx,
        )
        .child(
            Checkbox::new("jvm-flags-enabled")
                .label(t::server::settings::jvm_flags())
                .checked(jvm_flags_enabled)
                .on_click(cx.listener(|this, value: &bool, _, cx| {
                    this.jvm_flags_enabled = *value;
                    this.send_jvm_flags(cx);
                    cx.notify();
                })),
        )
        .child(Input::new(&self.jvm_flags).small().disabled(!jvm_flags_enabled))
        .child(
            Checkbox::new("java-enabled")
                .label(t::server::settings::java_custom())
                .checked(java_enabled)
                .on_click(cx.listener(|this, value: &bool, _, cx| {
                    this.java_enabled = *value;
                    this.send_java();
                    cx.notify();
                })),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .bg(theme.secondary)
                        .text_xs()
                        .font_family("Roboto Mono")
                        .truncate()
                        .when(!java_enabled, |this| this.opacity(0.5))
                        .child(java_label),
                )
                .child(
                    Button::new("pick-java")
                        .small()
                        .outline()
                        .icon(PandoraIcon::FolderOpen)
                        .label(t::server::settings::java_browse())
                        .on_click(cx.listener(|this, _, window, cx| this.pick_java(window, cx))),
                ),
        );

        let behaviour = card(
            PandoraIcon::RefreshCcw,
            t::server::settings::behaviour_title(),
            t::server::settings::behaviour_desc(),
            cx,
        )
        .child(
            h_flex()
                .gap_4()
                .child(
                    v_flex().flex_1().child(div().text_sm().child(t::server::settings::auto_restart())).child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t::server::settings::auto_restart_desc()),
                    ),
                )
                .child(Switch::new("auto-restart").checked(server.auto_restart).on_click(cx.listener(
                    move |this, value: &bool, _, _| {
                        this.backend_handle.send(MessageToBackend::SetServerAutoRestart {
                            id,
                            auto_restart: *value,
                        });
                    },
                ))),
        );

        let sync_target = self.sync_source.read(cx).selected_value().copied().flatten();
        // Only modded servers share mods with a client; plugins have no client-side counterpart
        let sync = server.platform.uses_mods().then(|| {
            card(
                PandoraIcon::GitBranch,
                t::server::settings::sync_title(),
                t::server::settings::sync_desc(),
                cx,
            )
            .when_some(server.linked_instance.clone(), |this, linked| {
                this.child(
                    h_flex()
                        .gap_1p5()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(PandoraIcon::Link).size_3p5())
                        .child(t::server::list::linked_to(&linked)),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Select::new(&self.sync_source).small().w_full()))
                    .child(
                        Button::new("review-sync")
                            .small()
                            .primary()
                            .icon(PandoraIcon::RefreshCcw)
                            .label(t::server::settings::sync_review())
                            .disabled(sync_target.is_none() || is_running)
                            .on_click({
                                let backend_handle = self.backend_handle.clone();
                                move |_, window, cx| {
                                    if let Some(client_id) = sync_target {
                                        crate::modals::server_sync::open_server_sync(
                                            id,
                                            client_id,
                                            backend_handle.clone(),
                                            window,
                                            cx,
                                        );
                                    }
                                }
                            }),
                    ),
            )
            .when(is_running, |this| {
                this.child(div().text_xs().text_color(theme.warning).child(t::server::settings::sync_stop_first()))
            })
        });

        let danger = v_flex()
            .gap_3()
            .p_4()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.danger.opacity(0.4))
            .child(
                h_flex()
                    .gap_4()
                    .child(
                        v_flex()
                            .flex_1()
                            .child(div().font_semibold().child(t::server::settings::delete_title()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(t::server::settings::delete_desc()),
                            ),
                    )
                    .child(
                        Button::new("delete-server")
                            .danger()
                            .small()
                            .icon(PandoraIcon::Trash2)
                            .label(t::server::action::delete())
                            .disabled(is_running)
                            .on_click({
                                let backend_handle = self.backend_handle.clone();
                                move |_, window, cx| {
                                    crate::modals::delete_instance::open_delete_instance(
                                        id,
                                        server_name.clone(),
                                        backend_handle.clone(),
                                        window,
                                        cx,
                                    );
                                }
                            }),
                    ),
            );

        div()
            .size_full()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .max_w(px(760.0))
                    .gap_4()
                    .p_4()
                    .child(eula)
                    .child(resources)
                    .child(java)
                    .child(behaviour)
                    .children(sync)
                    .child(danger),
            )
            .into_any_element()
    }
}
