use std::sync::Arc;

use bridge::{handle::BackendHandle, instance::InstanceID, message::MessageToBackend, modal_action::ModalAction};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, Icon, StyledExt, WindowExt,
    alert::Alert,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    dialog::Dialog,
    h_flex,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectState},
    skeleton::Skeleton,
    v_flex,
};
use schema::{
    loader::Loader,
    server::ServerPlatform,
    version_manifest::{MinecraftVersionManifest, MinecraftVersionType},
};

use crate::{
    component::named_dropdown::{DropdownName, NamedDropdown, NamedDropdownItem},
    entity::{
        instance::InstanceEntries,
        metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult, FrontendMetadataState},
    },
    icon::PandoraIcon,
    pages::instances_page::VersionList,
};

pub const EULA_URL: &str = "https://aka.ms/MinecraftEULA";

const PLATFORMS: &[ServerPlatform] = &[
    ServerPlatform::Vanilla,
    ServerPlatform::Paper,
    ServerPlatform::Purpur,
    ServerPlatform::Fabric,
    ServerPlatform::Forge,
    ServerPlatform::NeoForge,
];

/// One line under the platform picker saying what the platform is for, so someone who has never
/// run a server can still pick sensibly.
fn platform_blurb(platform: ServerPlatform) -> &'static str {
    match platform {
        ServerPlatform::Vanilla => t::server::platform::vanilla_desc(),
        ServerPlatform::Paper => t::server::platform::paper_desc(),
        ServerPlatform::Purpur => t::server::platform::purpur_desc(),
        ServerPlatform::Fabric => t::server::platform::fabric_desc(),
        ServerPlatform::Forge => t::server::platform::forge_desc(),
        ServerPlatform::NeoForge => t::server::platform::neoforge_desc(),
    }
}

#[derive(Clone, PartialEq)]
struct ClientSource {
    id: Option<InstanceID>,
    version: Option<SharedString>,
    loader: Option<Loader>,
}

struct CreateServerModalState {
    metadata: Entity<FrontendMetadata>,
    versions: Entity<FrontendMetadataState>,
    backend_handle: BackendHandle,
    version_dropdown: Entity<SelectState<VersionList>>,
    name_input_state: Entity<InputState>,
    source_dropdown: Entity<SelectState<NamedDropdown<ClientSource>>>,
    group_input_state: Entity<InputState>,
    platform: ServerPlatform,
    eula_accepted: bool,
    loaded_versions: bool,
    error_loading_versions: Option<SharedString>,
    name_invalid: bool,
    instance_names: Arc<[SharedString]>,
    _subscriptions: Vec<Subscription>,
}

impl CreateServerModalState {
    fn new(
        metadata: Entity<FrontendMetadata>,
        instances: Entity<InstanceEntries>,
        backend_handle: BackendHandle,
        generate_from: Option<InstanceID>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let instance_names: Arc<[SharedString]> =
            instances.read(cx).entries.values().map(|entry| entry.read(cx).name.clone()).collect();

        // Only client instances that actually have mods are worth generating a server from
        let mut sources = vec![NamedDropdownItem {
            name: DropdownName::translated(t::server::create::no_source),
            item: ClientSource {
                id: None,
                version: None,
                loader: None,
            },
        }];
        for entry in instances.read(cx).entries.values() {
            let entry = entry.read(cx);
            if entry.configuration.server.is_some()
                || entry.name == schema::quickplay::INSTANCE_NAME
                || entry.configuration.loader == Loader::Vanilla
            {
                continue;
            }
            sources.push(NamedDropdownItem {
                name: DropdownName::new(format!(
                    "{} ({} {})",
                    entry.name,
                    entry.configuration.loader.pretty_name(),
                    entry.configuration.minecraft_version
                )),
                item: ClientSource {
                    id: Some(entry.id),
                    version: Some(entry.configuration.minecraft_version.as_str().into()),
                    loader: Some(entry.configuration.loader),
                },
            });
        }
        let preselected_row = generate_from
            .and_then(|id| sources.iter().position(|source| source.item.id == Some(id)))
            .unwrap_or(0);

        let source_dropdown = cx.new(|cx| {
            SelectState::new(
                NamedDropdown::new(sources),
                Some(gpui_component::IndexPath::new(preselected_row)),
                window,
                cx,
            )
        });

        let version_dropdown = cx.new(|cx| SelectState::new(VersionList::default(), None, window, cx).searchable(true));

        let name_input_state =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::server::create::name_placeholder()));
        let group_input_state =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::server::create::group_placeholder()));

        let versions =
            FrontendMetadata::request(&metadata, bridge::meta::MetadataRequest::MinecraftVersionManifest, cx);

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe_in(&versions, window, |this, _, window, cx| {
            this.reload_version_dropdown(window, cx);
        }));
        {
            let instance_names = Arc::clone(&instance_names);
            subscriptions.push(cx.subscribe_in(
                &name_input_state,
                window,
                move |this, input_state, _: &InputEvent, _, cx| {
                    let text = input_state.read(cx).value();
                    this.name_invalid = !text.is_empty()
                        && (!crate::is_valid_instance_name(text.as_str()) || instance_names.contains(&text));
                    cx.notify();
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &source_dropdown,
            window,
            |this, _, event: &gpui_component::select::SelectEvent<NamedDropdown<ClientSource>>, window, cx| {
                if let gpui_component::select::SelectEvent::Confirm(Some(source)) = event {
                    this.apply_source(source.clone(), window, cx);
                }
            },
        ));

        let mut this = Self {
            metadata,
            versions,
            backend_handle,
            version_dropdown,
            name_input_state,
            source_dropdown,
            group_input_state,
            platform: ServerPlatform::Paper,
            eula_accepted: false,
            loaded_versions: false,
            error_loading_versions: None,
            name_invalid: false,
            instance_names,
            _subscriptions: subscriptions,
        };

        this.reload_version_dropdown(window, cx);
        if let Some(source) = this.source_dropdown.read(cx).selected_value().cloned() {
            this.apply_source(source, window, cx);
        }
        this
    }

    /// A server generated from a client instance has to match it, so picking one locks in its
    /// version and loader.
    fn apply_source(&mut self, source: ClientSource, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(loader) = source.loader {
            self.platform = match loader {
                Loader::Fabric => ServerPlatform::Fabric,
                Loader::Forge => ServerPlatform::Forge,
                Loader::NeoForge => ServerPlatform::NeoForge,
                Loader::Paper => ServerPlatform::Paper,
                Loader::Vanilla => ServerPlatform::Vanilla,
            };
        }
        if let Some(version) = source.version {
            self.version_dropdown.update(cx, |dropdown, cx| {
                dropdown.set_selected_value(&version, window, cx);
            });
        }
        cx.notify();
    }

    fn generate_from(&self, cx: &App) -> Option<InstanceID> {
        self.source_dropdown.read(cx).selected_value().and_then(|source| source.id)
    }

    fn reload_version_dropdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result: FrontendMetadataResult<MinecraftVersionManifest> = self.versions.read(cx).result();
        let (versions, latest) = match result {
            FrontendMetadataResult::Loading => {
                self.loaded_versions = false;
                self.error_loading_versions = None;
                (Vec::new(), None)
            },
            FrontendMetadataResult::Error(error, _) => {
                self.loaded_versions = false;
                self.error_loading_versions = Some(error);
                (Vec::new(), None)
            },
            FrontendMetadataResult::Loaded(manifest) => {
                self.loaded_versions = true;
                self.error_loading_versions = None;
                // Snapshots rarely have server builds outside vanilla, so they stay out of the list
                let versions: Vec<SharedString> = manifest
                    .versions
                    .iter()
                    .filter(|version| matches!(version.r#type, MinecraftVersionType::Release))
                    .map(|version| SharedString::from(version.id.as_str()))
                    .collect();
                (versions, Some(SharedString::from(manifest.latest.release.as_str())))
            },
        };

        // A server generated from an instance has to match its version, which wins over whatever
        // was picked before, even if the source was chosen before this list finished loading
        let source_version = self.source_dropdown.read(cx).selected_value().and_then(|source| source.version.clone());

        self.version_dropdown.update(cx, |dropdown, cx| {
            let previous = dropdown.selected_value().cloned();
            let to_select = source_version
                .filter(|version| versions.contains(version))
                .or(previous.filter(|previous| versions.contains(previous)))
                .or_else(|| latest.filter(|latest| versions.contains(latest)))
                .or_else(|| versions.first().cloned());

            dropdown.set_items(
                VersionList {
                    versions: versions.clone(),
                    matched_versions: versions,
                },
                window,
                cx,
            );
            if let Some(to_select) = to_select {
                dropdown.set_selected_value(&to_select, window, cx);
            }
        });
        cx.notify();
    }

    fn render_platform_picker(&self, locked: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.platform;
        let theme = cx.theme();

        let mut tiles = PLATFORMS.iter().enumerate().map(|(index, platform)| {
            let platform = *platform;
            let is_selected = platform == selected;
            let icon = match platform {
                ServerPlatform::Vanilla => PandoraIcon::Box,
                ServerPlatform::Paper => PandoraIcon::Feather,
                ServerPlatform::Purpur => PandoraIcon::Zap,
                ServerPlatform::Fabric => PandoraIcon::Layers,
                ServerPlatform::Forge => PandoraIcon::Anvil,
                ServerPlatform::NeoForge => PandoraIcon::Swords,
            };

            div()
                .id(("platform", index))
                .flex_1()
                .min_w_20()
                .p_2()
                .gap_1()
                .flex()
                .flex_col()
                .items_center()
                .rounded(theme.radius)
                .border_1()
                .border_color(if is_selected { theme.primary } else { theme.border })
                .when(is_selected, |this| this.bg(theme.primary.opacity(0.12)))
                .when(!locked, |this| {
                    this.cursor_pointer().hover(|this| this.bg(theme.secondary_hover)).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.platform = platform;
                            cx.notify();
                        },
                    ))
                })
                .when(locked && !is_selected, |this| this.opacity(0.4))
                .child(Icon::new(icon).size_5().text_color(if is_selected {
                    theme.primary
                } else {
                    theme.muted_foreground
                }))
                .child(div().text_xs().font_medium().child(platform.pretty_name()))
        });

        v_flex()
            .gap_1p5()
            // Two even rows of three, rather than letting the last tile wrap onto a row of its own
            .child(h_flex().gap_2().children(tiles.by_ref().take(3)))
            .child(h_flex().gap_2().children(tiles))
            .child(div().text_xs().text_color(theme.muted_foreground).child(platform_blurb(selected)))
    }

    fn render(&mut self, dialog: Dialog, _window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        if let Some(error) = self.error_loading_versions.clone() {
            let metadata = self.metadata.clone();
            return dialog
                .title(t::server::create::title())
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            Alert::new("error", error)
                                .icon(PandoraIcon::CircleX)
                                .title(t::instance::versions_loading::error()),
                        )
                        .child(
                            Button::new("reload-versions")
                                .primary()
                                .label(t::instance::versions_loading::reload())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.error_loading_versions = None;
                                    FrontendMetadata::force_reload(
                                        &metadata,
                                        bridge::meta::MetadataRequest::MinecraftVersionManifest,
                                        cx,
                                    );
                                })),
                        ),
                )
                .footer(Button::new("ok").label(t::common::ok()).on_click(|_, window, cx| window.close_dialog(cx)));
        }

        let generating = self.generate_from(cx).is_some();
        let theme = cx.theme();

        let version_select = if self.loaded_versions {
            Select::new(&self.version_dropdown)
                .w_full()
                .search_placeholder(t::common::search())
                .disabled(generating)
                .into_any_element()
        } else {
            Skeleton::new().w_full().min_h_8().max_h_8().rounded_md().into_any_element()
        };

        let eula = h_flex()
            .gap_2()
            .p_3()
            .rounded(theme.radius)
            .border_1()
            .border_color(if self.eula_accepted {
                theme.border
            } else {
                theme.warning.opacity(0.6)
            })
            .bg(if self.eula_accepted {
                theme.transparent
            } else {
                theme.warning.opacity(0.06)
            })
            .child(
                Checkbox::new("eula")
                    .checked(self.eula_accepted)
                    .on_click(cx.listener(|this, value, _, cx| {
                        this.eula_accepted = *value;
                        cx.notify();
                    })),
            )
            .child(
                v_flex().text_sm().child(t::server::eula::agree()).child(
                    div()
                        .id("eula-link")
                        .text_xs()
                        .text_color(theme.link)
                        .cursor_pointer()
                        .hover(|this| this.underline())
                        .child(t::server::eula::read())
                        .on_click(|_, _, cx| cx.open_url(EULA_URL)),
                ),
            );

        let content = v_flex()
            .gap_4()
            .child(crate::labelled(t::instance::name(), Input::new(&self.name_input_state)))
            .child(crate::labelled(
                t::server::create::source(),
                v_flex().gap_1().child(Select::new(&self.source_dropdown).w_full()).child(
                    div().text_xs().text_color(theme.muted_foreground).child(if generating {
                        t::server::create::source_selected_desc()
                    } else {
                        t::server::create::source_desc()
                    }),
                ),
            ))
            .child(crate::labelled(t::server::platform::title(), self.render_platform_picker(generating, cx)))
            .child(crate::labelled(t::instance::mc_version(), version_select))
            .child(crate::labelled(t::server::create::group(), Input::new(&self.group_input_state)))
            .child(eula);

        let can_create = self.eula_accepted && !self.name_invalid && self.loaded_versions;

        dialog
            .title(t::server::create::title())
            .overlay_closable(false)
            .w(px(520.0))
            .child(content)
            .footer(
                h_flex()
                    .gap_2()
                    .w_full()
                    .child(
                        Button::new("cancel")
                            .flex_1()
                            .label(t::common::cancel())
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("create")
                            .flex_1()
                            .success()
                            .icon(PandoraIcon::Server)
                            .label(t::server::create::action())
                            .disabled(!can_create)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit(window, cx);
                            })),
                    ),
            )
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.eula_accepted || self.name_invalid {
            return;
        }
        let Some(version) = self.version_dropdown.read(cx).selected_value().cloned() else {
            return;
        };

        let typed_name = self.name_input_state.read(cx).value().trim().to_string();
        let name = if typed_name.is_empty() {
            unique_name(&format!("{} {} Server", self.platform.pretty_name(), version), &self.instance_names)
        } else {
            typed_name
        };

        let group = self.group_input_state.read(cx).value().clone();
        let group = group.trim();
        let group = (!group.is_empty()).then(|| Arc::<str>::from(group));

        let modal_action = ModalAction::default();
        self.backend_handle.send(MessageToBackend::CreateServerInstance {
            name: name.as_str().into(),
            version: version.as_str().into(),
            platform: self.platform,
            eula_accepted: self.eula_accepted,
            group,
            generate_from: self.generate_from(cx),
            modal_action: modal_action.clone(),
        });

        window.close_dialog(cx);
        crate::modals::generic::show_notification(window, cx, t::server::create::error().into(), modal_action);
    }
}

fn unique_name(base: &str, existing: &[SharedString]) -> String {
    let sanitized = sanitize_filename::sanitize_with_options(
        base,
        sanitize_filename::Options {
            windows: true,
            ..Default::default()
        },
    );
    if !existing.iter().any(|name| name.as_str() == sanitized) {
        return sanitized;
    }
    for n in 2.. {
        let candidate = format!("{sanitized} ({n})");
        if !existing.iter().any(|name| name.as_str() == candidate) {
            return candidate;
        }
    }
    unreachable!()
}

pub fn open_create_server(
    metadata: Entity<FrontendMetadata>,
    instances: Entity<InstanceEntries>,
    backend_handle: BackendHandle,
    generate_from: Option<InstanceID>,
    window: &mut Window,
    cx: &mut App,
) {
    let state =
        cx.new(|cx| CreateServerModalState::new(metadata, instances, backend_handle, generate_from, window, cx));

    window.open_dialog(cx, move |dialog, window, cx| {
        cx.update_entity(&state, |state, cx| state.render(dialog, window, cx))
    });
}
