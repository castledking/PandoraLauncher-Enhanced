use std::sync::Arc;

use bridge::{handle::BackendHandle, instance::InstanceID, message::MessageToBackend, modal_action::ModalAction};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, WindowExt,
    button::Button,
    checkbox::Checkbox,
    dialog::Dialog,
    h_flex,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectState},
    v_flex,
};
use schema::version_manifest::{MinecraftVersionManifest, MinecraftVersionType};

use crate::{
    entity::{
        instance::InstanceEntries,
        metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult, FrontendMetadataState},
    },
    get_unique_instance_name,
    interface_config::InterfaceConfig,
    is_valid_instance_name,
    modals::generic,
    pages::instances_page::VersionList,
};

struct DuplicateInstanceModalState {
    instance_id: InstanceID,
    backend_handle: BackendHandle,
    name_input_state: Entity<InputState>,
    name_invalid: bool,
    default_name: SharedString,
    current_version: SharedString,
    versions: Entity<FrontendMetadataState>,
    version_dropdown: Entity<SelectState<VersionList>>,
    _name_input_subscription: Subscription,
    _versions_updated_subscription: Subscription,
    _version_selected_subscription: Subscription,
}

impl DuplicateInstanceModalState {
    pub fn new(
        instance_id: InstanceID,
        instance_name: SharedString,
        current_version: SharedString,
        instances: Entity<InstanceEntries>,
        metadata: Entity<FrontendMetadata>,
        backend_handle: BackendHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let instance_names: Arc<[SharedString]> =
            instances.read(cx).entries.iter().map(|(_, v)| v.read(cx).name.clone()).collect();

        let instance_name_strings: Vec<&str> = instance_names.iter().map(|s| s.as_str()).collect();
        let default_name = SharedString::from(get_unique_instance_name(
            &t::instance::duplicate::copy_of(&instance_name),
            &instance_name_strings,
        ));

        let name_input_state = cx.new(|cx| InputState::new(window, cx).placeholder(default_name.clone()));

        let _name_input_subscription = {
            let instance_names = Arc::clone(&instance_names);
            cx.subscribe_in(&name_input_state, window, move |this, input_state, _: &InputEvent, _, cx| {
                let text = input_state.read(cx).value();
                let resolved = if text.is_empty() {
                    this.default_name.as_str()
                } else {
                    text.as_str()
                };
                if resolved.is_empty() || !is_valid_instance_name(resolved) {
                    this.name_invalid = true;
                    return;
                }
                this.name_invalid = instance_names.contains(&text);
            })
        };

        let version_dropdown = cx.new(|cx| SelectState::new(VersionList::default(), None, window, cx).searchable(true));
        let _version_selected_subscription = cx.observe(&version_dropdown, |_, _, cx| cx.notify());

        let versions =
            FrontendMetadata::request(&metadata, bridge::meta::MetadataRequest::MinecraftVersionManifest, cx);
        let _versions_updated_subscription = cx.observe_in(&versions, window, |this, _, window, cx| {
            this.reload_version_dropdown(window, cx);
        });

        let mut this = Self {
            instance_id,
            backend_handle,
            name_input_state,
            name_invalid: false,
            default_name,
            current_version,
            versions,
            version_dropdown,
            _name_input_subscription,
            _versions_updated_subscription,
            _version_selected_subscription,
        };
        this.reload_version_dropdown(window, cx);
        this
    }

    /// The instance's own version is always offered and picked first, so a plain duplicate is just
    /// a matter of leaving the dropdown alone, even before the version list has loaded.
    fn reload_version_dropdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let show_snapshots = InterfaceConfig::get(cx).show_snapshots_in_create_instance;
        let result: FrontendMetadataResult<MinecraftVersionManifest> = self.versions.read(cx).result();
        let mut versions: Vec<SharedString> = match result {
            FrontendMetadataResult::Loaded(manifest) => manifest
                .versions
                .iter()
                .filter(|version| {
                    show_snapshots
                        || !matches!(version.r#type, MinecraftVersionType::Snapshot)
                        || version.id.as_str() == self.current_version.as_str()
                })
                .map(|version| SharedString::from(version.id.as_str()))
                .collect(),
            FrontendMetadataResult::Loading | FrontendMetadataResult::Error(..) => Vec::new(),
        };
        if !versions.contains(&self.current_version) {
            versions.insert(0, self.current_version.clone());
        }

        let current_version = self.current_version.clone();
        self.version_dropdown.update(cx, |dropdown, cx| {
            let to_select = dropdown
                .selected_value()
                .cloned()
                .filter(|selected| versions.contains(selected))
                .unwrap_or(current_version);

            dropdown.set_items(
                VersionList {
                    versions: versions.clone(),
                    matched_versions: versions,
                },
                window,
                cx,
            );
            dropdown.set_selected_value(&to_select, window, cx);
        });
        cx.notify();
    }

    /// The version to move the copy to, if it's not the one the instance is already on.
    fn target_version(&self, cx: &App) -> Option<SharedString> {
        self.version_dropdown
            .read(cx)
            .selected_value()
            .filter(|version| **version != self.current_version)
            .cloned()
    }

    pub fn render(&mut self, dialog: Dialog, _window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        let target_version = self.target_version(cx);

        let version_picker = v_flex()
            .gap_2()
            .child(Select::new(&self.version_dropdown).w_full().search_placeholder(t::common::search()))
            .child(
                Checkbox::new("show_snapshots")
                    .checked(InterfaceConfig::get(cx).show_snapshots_in_create_instance)
                    .label(t::instance::show_snapshots())
                    .on_click(cx.listener(|this, show, window, cx| {
                        InterfaceConfig::get_mut(cx).show_snapshots_in_create_instance = *show;
                        this.reload_version_dropdown(window, cx);
                    })),
            )
            .when_some(target_version.clone(), |this, version| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t::instance::duplicate::version_hint(&version)),
                )
            });

        let content = v_flex()
            .gap_3()
            .child(crate::labelled(
                t::instance::name(),
                Input::new(&self.name_input_state).when(self.name_invalid, |this| this.border_color(cx.theme().danger)),
            ))
            .child(crate::labelled(t::instance::version(), version_picker));

        let name_is_invalid = self.name_invalid;
        dialog
            .overlay_closable(false)
            .title(t::instance::duplicate::title())
            .child(content)
            .when(name_is_invalid, |dialog| {
                dialog.footer(
                    h_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            Button::new("cancel")
                                .flex_1()
                                .label(t::common::cancel())
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(Button::new("ok").flex_1().opacity(0.5).label(t::common::ok())),
                )
            })
            .when(!name_is_invalid, |dialog| {
                dialog.footer(
                    h_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            Button::new("cancel")
                                .flex_1()
                                .label(t::common::cancel())
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(Button::new("ok").flex_1().label(t::common::ok()).on_click(cx.listener(
                            move |this, _, window, cx| {
                                let mut name = this.name_input_state.read(cx).value().clone();
                                if name.is_empty() {
                                    name = this.default_name.clone();
                                }

                                let backend_handle = this.backend_handle.clone();
                                let instance_id = this.instance_id;
                                let version = this.target_version(cx);
                                let modal_action = ModalAction::default();

                                window.close_dialog(cx);

                                generic::show_modal(
                                    window,
                                    cx,
                                    t::instance::duplicate::progress().into(),
                                    t::instance::duplicate::error().into(),
                                    modal_action.clone(),
                                );

                                backend_handle.send(MessageToBackend::DuplicateInstance {
                                    id: instance_id,
                                    name: name.as_str().into(),
                                    version: version.map(|version| version.as_str().into()),
                                    modal_action,
                                });
                            },
                        ))),
                )
            })
    }
}

pub fn open_duplicate_instance(
    instance_id: InstanceID,
    instance_name: SharedString,
    current_version: SharedString,
    instances: Entity<InstanceEntries>,
    metadata: Entity<FrontendMetadata>,
    backend_handle: BackendHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let state = cx.new(|cx| {
        DuplicateInstanceModalState::new(
            instance_id,
            instance_name,
            current_version,
            instances,
            metadata,
            backend_handle,
            window,
            cx,
        )
    });

    window.open_dialog(cx, move |modal, window, cx| {
        cx.update_entity(&state, |state, cx| state.render(modal, window, cx))
    });
}
