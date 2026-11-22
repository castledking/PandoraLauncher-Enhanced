use std::{path::Path, sync::Arc};

use bridge::{handle::BackendHandle, instance::InstanceID, message::MessageToBackend, modal_action::ModalAction};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IndexPath, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState, NumberInput, NumberInputEvent, StepAction},
    scroll::ScrollableElement,
    select::{Select, SelectEvent, SelectState},
    skeleton::Skeleton,
    switch::Switch,
    v_flex,
};
use rustc_hash::FxHashMap;
use schema::{
    forge::VersionFragment,
    instance::{
        InstanceJavaRuntimeConfiguration, InstanceJvmBinaryConfiguration, InstanceJvmFlagsConfiguration,
        InstanceMemoryConfiguration,
    },
    loader::Loader,
    server::ServerPlatform,
};
use ustr::Ustr;

use crate::{
    component::{
        java_runtime_picker::{self, JavaRuntimeChoice, JavaRuntimeSelect},
        named_dropdown::{DropdownName, NamedDropdown, NamedDropdownItem},
    },
    entity::{DataEntities, instance::InstanceEntry},
    icon::PandoraIcon,
    pages::{instances_page::VersionList, servers_page::server_details},
};

/// Servers need more than a client by default, so the fields start somewhere useful rather than
/// at the client defaults.
const DEFAULT_MIN_MEMORY: u32 = 1024;
const DEFAULT_MAX_MEMORY: u32 = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
enum VersionsState {
    Loading,
    Loaded,
    Failed,
}

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
    java_runtime_enabled: bool,
    java_runtime_select: Entity<JavaRuntimeSelect>,
    sync_source: Entity<SelectState<NamedDropdown<Option<InstanceID>>>>,
    /// The platform picked in the Server software card, which only takes effect once switched to.
    software_platform: ServerPlatform,
    software_version: Entity<SelectState<VersionList>>,
    /// Kept for as long as the page is open, so flicking between platforms doesn't refetch.
    platform_versions: FxHashMap<ServerPlatform, Arc<[Ustr]>>,
    versions_state: VersionsState,
    _platform_versions_task: Task<()>,
    _select_java_task: Task<()>,
    _select_jar_task: Task<()>,
    _java_runtimes_task: Task<()>,
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
        let java_runtime = configuration.java_runtime.clone().unwrap_or_default();

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

        let software_platform = configuration.server.as_ref().map(|server| server.platform).unwrap_or_default();
        let software_version = cx.new(|cx| SelectState::new(VersionList::default(), None, window, cx).searchable(true));

        let java_runtime_select = NamedDropdown::create_and_select(
            vec![java_runtime_picker::default_choice_item()],
            java_runtime.component,
            window,
            cx,
        );

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
            cx.subscribe(&software_version, |_, _, _: &SelectEvent<VersionList>, cx| {
                cx.notify();
            }),
            cx.subscribe(&java_runtime_select, |this, _, event: &SelectEvent<_>, _cx| {
                let SelectEvent::Confirm(Some(component)) = event else {
                    return;
                };
                this.send_java_runtime(*component);
            }),
            cx.observe(instance, |_, _, cx| cx.notify()),
        ];

        let mut this = Self {
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
            java_runtime_enabled: java_runtime.enabled,
            java_runtime_select,
            sync_source,
            software_platform,
            software_version,
            platform_versions: FxHashMap::default(),
            versions_state: VersionsState::Loading,
            _platform_versions_task: Task::ready(()),
            _select_java_task: Task::ready(()),
            _select_jar_task: Task::ready(()),
            _java_runtimes_task: Task::ready(()),
            _subscriptions: subscriptions,
        }
        .load_java_runtimes(window, cx);
        this.load_platform_versions(window, cx);
        this
    }

    /// Fills the version picker with what the picked platform has servers for, asking the
    /// backend the first time each platform is picked.
    fn load_platform_versions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let platform = self.software_platform;
        if self.platform_versions.contains_key(&platform) {
            self.versions_state = VersionsState::Loaded;
            self.fill_version_select(window, cx);
            return;
        }

        self.versions_state = VersionsState::Loading;
        let (send, recv) = tokio::sync::oneshot::channel();
        self.backend_handle.send(MessageToBackend::GetServerPlatformVersions {
            platform,
            channel: send,
        });
        // Replacing the task drops the answer for a platform that has since been picked away from
        self._platform_versions_task = cx.spawn_in(window, async move |this, cx| {
            let versions = recv.await.ok().flatten();
            _ = this.update_in(cx, |this, window, cx| match versions {
                Some(versions) => {
                    this.platform_versions.insert(platform, versions);
                    this.versions_state = VersionsState::Loaded;
                    this.fill_version_select(window, cx);
                },
                None => {
                    this.versions_state = VersionsState::Failed;
                    cx.notify();
                },
            });
        });
        cx.notify();
    }

    fn fill_version_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.instance.read(cx);
        let current_version = SharedString::from(entry.configuration.minecraft_version.as_str());
        let current_platform = entry.configuration.server.as_ref().map(|server| server.platform);

        let mut versions: Vec<SharedString> = self
            .platform_versions
            .get(&self.software_platform)
            .map(|versions| versions.iter().map(|version| SharedString::from(version.as_str())).collect())
            .unwrap_or_default();
        // A server always offers its own version on its own platform, even a snapshot the list
        // leaves out, so the picker never claims it runs something else
        if current_platform == Some(self.software_platform) && !versions.contains(&current_version) {
            versions.insert(0, current_version.clone());
        }

        self.software_version.update(cx, |select, cx| {
            // Moving between platforms keeps the picked version where the new one has it too
            let to_select = select
                .selected_value()
                .cloned()
                .filter(|version| versions.contains(version))
                .or_else(|| Some(current_version).filter(|version| versions.contains(version)))
                .or_else(|| versions.first().cloned());

            select.set_items(
                VersionList {
                    versions: versions.clone(),
                    matched_versions: versions,
                },
                window,
                cx,
            );
            match to_select {
                Some(version) => select.set_selected_value(&version, window, cx),
                None => select.set_selected_index(None, window, cx),
            }
        });
        cx.notify();
    }

    fn pick_platform(&mut self, platform: ServerPlatform, window: &mut Window, cx: &mut Context<Self>) {
        if self.software_platform == platform {
            return;
        }
        self.software_platform = platform;
        self.load_platform_versions(window, cx);
    }

    /// The version picked for the picked platform, if the picker has one from its current list.
    fn picked_version(&self, cx: &App) -> Option<SharedString> {
        if self.versions_state != VersionsState::Loaded {
            return None;
        }
        self.software_version.read(cx).selected_value().cloned()
    }

    fn switch_software(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(version) = self.picked_version(cx) else {
            return;
        };
        let modal_action = ModalAction::default();
        self.backend_handle.send(MessageToBackend::SetServerSoftware {
            id: self.instance_id,
            platform: self.software_platform,
            version: version.as_str().into(),
            modal_action: modal_action.clone(),
        });
        crate::modals::generic::show_notification(window, cx, t::server::software::error().into(), modal_action);
    }

    /// Fills the runtime picker with what the backend has to offer. Done once when the page
    /// opens, since the list only changes when Mojang publishes a new runtime.
    fn load_java_runtimes(mut self, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = self.java_runtime_select.clone();
        let backend_handle = self.backend_handle.clone();
        let window_handle = window.window_handle();

        self._java_runtimes_task = cx.spawn(async move |_, cx| {
            let runtimes = java_runtime_picker::request_runtimes(&backend_handle).await.unwrap_or_default();
            _ = cx.update_window(window_handle, |_, window, cx| {
                java_runtime_picker::fill_select(&state, &runtimes, window, cx);
            });
        });

        self
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

    fn send_java_runtime(&self, component: JavaRuntimeChoice) {
        self.backend_handle.send(MessageToBackend::SetInstanceJavaRuntime {
            id: self.instance_id,
            java_runtime: InstanceJavaRuntimeConfiguration {
                enabled: self.java_runtime_enabled,
                component,
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

    /// The backend copies the picked jar into the server and switches to it.
    fn pick_custom_jar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t::server::jar::pick().into()),
        });
        let backend_handle = self.backend_handle.clone();
        let id = self.instance_id;
        self._select_jar_task = cx.spawn_in(window, async move |_, _| {
            let Ok(Ok(Some(mut paths))) = receiver.await else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            backend_handle.send(MessageToBackend::SetServerCustomJar {
                id,
                jar: paths.swap_remove(0),
            });
        });
    }
}

impl ServerSettingsSubpage {
    /// Platform tiles and a version picker, so moving a server to other software is a pick and a
    /// click instead of finding and downloading a jar by hand.
    fn render_software(&self, cx: &mut Context<Self>) -> Option<Div> {
        let entry = self.instance.read(cx);
        let server = entry.configuration.server.clone()?;
        let current_version = entry.configuration.minecraft_version;
        let is_running = entry.status != bridge::instance::InstanceStatus::NotRunning;
        let theme = cx.theme().clone();

        let current_platform = server.platform;
        let picked_platform = self.software_platform;
        let picked_version = self.picked_version(cx);
        let changed = picked_version
            .as_ref()
            .is_some_and(|version| picked_platform != current_platform || version.as_str() != current_version.as_str());

        let entity = cx.entity().downgrade();
        let tiles =
            crate::modals::create_server::platform_picker(picked_platform, false, cx, move |platform, window, cx| {
                _ = entity.update(cx, |this, cx| this.pick_platform(platform, window, cx));
            });

        let version_picker = match self.versions_state {
            VersionsState::Loading => Skeleton::new().w_full().min_h_8().max_h_8().rounded_md().into_any_element(),
            VersionsState::Failed => h_flex()
                .gap_2()
                .child(div().flex_1().text_xs().text_color(theme.danger).child(t::server::software::load_error()))
                .child(
                    Button::new("retry-versions")
                        .small()
                        .outline()
                        .label(t::server::software::retry())
                        .on_click(cx.listener(|this, _, window, cx| this.load_platform_versions(window, cx))),
                )
                .into_any_element(),
            VersionsState::Loaded if self.software_version.read(cx).selected_value().is_none() => div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(t::server::software::none(picked_platform.pretty_name()))
                .into_any_element(),
            VersionsState::Loaded => Select::new(&self.software_version)
                .small()
                .w_full()
                .search_placeholder(t::common::search())
                .into_any_element(),
        };

        // What the switch leaves behind, said before it happens rather than discovered after
        let mut notes: Vec<(bool, SharedString)> = Vec::new();
        if changed {
            let picked_name = picked_platform.pretty_name();
            if is_running {
                notes.push((true, t::server::software::stop_first().into()));
            }
            if let Some(version) = &picked_version
                && VersionFragment::string_to_parts(version) < VersionFragment::string_to_parts(&current_version)
            {
                notes.push((true, t::server::software::downgrade(version).into()));
            }
            if current_platform.uses_plugins() && !picked_platform.uses_plugins() {
                notes.push((false, t::server::software::plugins_left(picked_name).into()));
                notes.push((true, t::server::software::bukkit_worlds(picked_name).into()));
            }
            if current_platform.uses_mods() && !picked_platform.uses_mods() {
                notes.push((false, t::server::software::mods_left(picked_name).into()));
            }
            if current_platform.uses_mods()
                && picked_platform.uses_mods()
                && current_platform.loader() != picked_platform.loader()
            {
                notes.push((
                    false,
                    t::server::software::other_loader(current_platform.pretty_name(), picked_name).into(),
                ));
            }
            if server.active_custom_jar().is_some() {
                notes.push((false, t::server::software::custom_jar_off().into()));
            }
        }

        let switch_label: SharedString = match (&picked_version, changed) {
            (Some(version), true) => {
                t::server::software::switch(&format!("{} {version}", picked_platform.pretty_name())).into()
            },
            _ => t::server::software::switch_plain().into(),
        };

        Some(
            card(PandoraIcon::Server, t::server::software::title(), t::server::software::desc(), cx)
                .child(tiles)
                .child(crate::labelled(t::server::software::version(), version_picker))
                .children(notes.into_iter().map(|(warning, note)| {
                    h_flex()
                        .gap_1p5()
                        .items_start()
                        .text_xs()
                        .text_color(if warning { theme.warning } else { theme.muted_foreground })
                        .child(
                            Icon::new(if warning {
                                PandoraIcon::TriangleAlert
                            } else {
                                PandoraIcon::Info
                            })
                            .size_3p5()
                            .flex_shrink_0()
                            .mt_px(),
                        )
                        .child(div().flex_1().min_w_0().child(note))
                }))
                .child(
                    h_flex()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(t::server::software::current(&server_details(&server, &current_version))),
                        )
                        .child(
                            Button::new("switch-software")
                                .small()
                                .primary()
                                .icon(PandoraIcon::ArrowLeftRight)
                                .label(switch_label)
                                .disabled(!changed || is_running)
                                .on_click(cx.listener(|this, _, window, cx| this.switch_software(window, cx))),
                        ),
                ),
        )
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
                        .flex_1()
                        .min_w_0()
                        .child(div().font_semibold().child(title))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(description)),
                ),
        )
}

impl Render for ServerSettingsSubpage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let software = self.render_software(cx);
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

        let custom_jar_active = server.active_custom_jar().is_some();
        let has_custom_jar = server.custom_jar.is_some();
        let jar_label: SharedString = match &server.custom_jar {
            Some(jar) => jar.file.to_string().into(),
            None => t::server::jar::none().into(),
        };
        let jar = card(PandoraIcon::PackageOpen, t::server::jar::title(), t::server::jar::desc(), cx)
            .child(
                Checkbox::new("custom-jar")
                    .label(t::server::jar::custom())
                    .checked(custom_jar_active)
                    .on_click(cx.listener(move |this, value: &bool, window, cx| {
                        // Switching it on with nothing picked yet goes straight to picking one
                        if has_custom_jar {
                            this.backend_handle
                                .send(MessageToBackend::SetServerCustomJarEnabled { id, enabled: *value });
                        } else if *value {
                            this.pick_custom_jar(window, cx);
                        }
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
                            .when(!custom_jar_active, |this| this.opacity(0.5))
                            .child(jar_label),
                    )
                    .child(
                        Button::new("pick-jar")
                            .small()
                            .outline()
                            .icon(PandoraIcon::FolderOpen)
                            .label(t::server::jar::browse())
                            .on_click(cx.listener(|this, _, window, cx| this.pick_custom_jar(window, cx))),
                    ),
            )
            .child(div().text_xs().opacity(0.6).child(t::server::jar::custom_desc()));

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
        let java_runtime_enabled = self.java_runtime_enabled;
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
        )
        .child(
            Checkbox::new("java-runtime-enabled")
                .label(t::server::settings::java_runtime())
                .checked(java_runtime_enabled)
                .on_click(cx.listener(|this, value: &bool, _, cx| {
                    this.java_runtime_enabled = *value;
                    let component = this.java_runtime_select.read(cx).selected_value().copied().flatten();
                    this.send_java_runtime(component);
                    cx.notify();
                })),
        )
        .child(
            div().flex_1().min_w_0().child(
                Select::new(&self.java_runtime_select)
                    .small()
                    .w_full()
                    .placeholder(t::settings::java::runtime::loading())
                    .disabled(!java_runtime_enabled),
            ),
        )
        .child(div().text_xs().opacity(0.6).child(t::server::settings::java_runtime_desc()));

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
                                move |click: &ClickEvent, window, cx| {
                                    crate::component::instance_list::delete_instance(
                                        id,
                                        &server_name,
                                        &backend_handle,
                                        click.modifiers().shift,
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
                    .children(software)
                    .child(jar)
                    .child(resources)
                    .child(java)
                    .child(behaviour)
                    .children(sync)
                    .child(danger),
            )
            .into_any_element()
    }
}
