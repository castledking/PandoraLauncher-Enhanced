use bridge::{
    handle::BackendHandle,
    instance::{InstanceID, InstanceStatus},
    message::MessageToBackend,
};
use gpui::{prelude::*, *};
use gpui_component::{
    WindowExt,
    button::{Button, ButtonGroup, ButtonVariants},
    h_flex,
    tab::{Tab, TabBar},
    v_flex,
};
use serde::{Deserialize, Serialize};

use crate::{
    entity::{DataEntities, instance::InstanceEntry},
    game_output::GameOutputRoot,
    icon::PandoraIcon,
    interface_config::InterfaceConfig,
    pages::{
        instance::{
            content_subpage::InstanceContentSubpage, logs_subpage::InstanceLogsSubpage,
            quickplay_subpage::InstanceQuickplaySubpage, settings_subpage::InstanceSettingsSubpage,
        },
        page::Page,
    },
    root,
};

use super::content_subpage::ContentType;

pub struct InstancePage {
    data: DataEntities,
    pub instance: Entity<InstanceEntry>,
    subpage: InstanceSubpage,
}

impl InstancePage {
    pub fn new(instance_id: InstanceID, data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let instance = data.instances.read(cx).entries.get(&instance_id).unwrap().clone();

        let instance_subpage = InterfaceConfig::get(cx).instance_subpage;
        let subpage = instance_subpage.create(&instance, data, data.backend_handle.clone(), window, cx);

        let subpage = subpage.unwrap_or_else(|| {
            InterfaceConfig::get_mut(cx).instance_subpage = InstanceSubpageType::Quickplay;
            InstanceSubpageType::Quickplay
                .create(&instance, data, data.backend_handle.clone(), window, cx)
                .unwrap()
        });

        Self {
            data: data.clone(),
            instance,
            subpage,
        }
    }
}

impl Page for InstancePage {
    fn controls(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let instance = self.instance.read(cx);
        let id = instance.id;
        let name = instance.name.clone();
        let data = self.data.clone();

        let button =
            match instance.status {
                InstanceStatus::NotRunning => Button::new("start_instance")
                    .success()
                    .icon(PandoraIcon::Play)
                    .label(t::instance::start::label())
                    .on_click(move |_, window, cx| {
                        root::start_instance(id, name.clone(), None, &data, window, cx);
                    })
                    .into_any_element(),
                InstanceStatus::Launching => Button::new("launching")
                    .warning()
                    .icon(PandoraIcon::Loader)
                    .label(t::instance::start::starting())
                    .into_any_element(),
                InstanceStatus::Stopping => Button::new("stopping")
                    .danger()
                    .icon(PandoraIcon::Loader)
                    .label(t::instance::start::stopping())
                    .on_click({
                        let backend_handle = data.backend_handle.clone();
                        move |_, _, _| {
                            backend_handle.send(MessageToBackend::KillInstance { id });
                        }
                    })
                    .into_any_element(),
                InstanceStatus::Running => ButtonGroup::new("running")
                    .child(
                        Button::new("kill_instance")
                            .danger()
                            .icon(PandoraIcon::Close)
                            .label(t::instance::kill_instance())
                            .on_click({
                                let backend_handle = data.backend_handle.clone();
                                move |_, _, _| {
                                    backend_handle.send(MessageToBackend::KillInstance { id });
                                }
                            }),
                    )
                    .child(Button::new("start_again").success().icon(PandoraIcon::Play).on_click(
                        move |_, window, cx| {
                            let name = name.clone();
                            let data = data.clone();
                            window.open_dialog(cx, move |dialog, _, _| {
                                dialog
                                    .title(t::instance::already_running::title())
                                    .overlay_closable(false)
                                    .flex()
                                    .line_height(rems(1.2))
                                    .child(t::instance::already_running::body())
                                    .child(div().h_2())
                                    .child(t::instance::already_running::body2())
                                    .footer(
                                        h_flex()
                                            .gap_2()
                                            .w_full()
                                            .child(
                                                Button::new("cancel")
                                                    .label(t::common::cancel())
                                                    .on_click(|_, window, cx| {
                                                        window.close_dialog(cx);
                                                    })
                                                    .flex_grow(1.0),
                                            )
                                            .child(
                                                Button::new("ok")
                                                    .success()
                                                    .label(t::instance::already_running::start_anyway())
                                                    .on_click({
                                                        let name = name.clone();
                                                        let data = data.clone();
                                                        move |_, window, cx| {
                                                            window.close_dialog(cx);
                                                            root::start_instance(
                                                                id,
                                                                name.clone(),
                                                                None,
                                                                &data,
                                                                window,
                                                                cx,
                                                            );
                                                        }
                                                    }),
                                            ),
                                    )
                            })
                        },
                    ))
                    .into_any_element(),
            };

        let open_dot_minecraft_button = Button::new("open_dot_minecraft")
            .info()
            .icon(PandoraIcon::FolderOpen)
            .label(t::instance::open_folder())
            .on_click({
                let dot_minecraft = instance.dot_minecraft_folder.clone();
                move |_, window, cx| {
                    crate::open_folder(&dot_minecraft, window, cx);
                }
            });

        h_flex().gap_3().child(button).child(open_dot_minecraft_button)
    }

    fn scrollable(&self, _cx: &App) -> bool {
        false
    }
}

impl Render for InstancePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let instance_subpage = InterfaceConfig::get(cx).instance_subpage;
        if instance_subpage != self.subpage.page_type() {
            let subpage =
                instance_subpage.create(&self.instance, &self.data, self.data.backend_handle.clone(), window, cx);

            self.subpage = subpage.unwrap_or_else(|| {
                InterfaceConfig::get_mut(cx).instance_subpage = InstanceSubpageType::Quickplay;
                InstanceSubpageType::Quickplay
                    .create(&self.instance, &self.data, self.data.backend_handle.clone(), window, cx)
                    .unwrap()
            });
        }

        let entry = self.instance.read(cx);
        let show_live_game_output = entry.live_game_output.is_some();

        if let InstanceSubpage::LiveGameOutput(current_output) = &self.subpage
            && let Some(desired_output) = &entry.live_game_output
            && current_output != desired_output
        {
            self.subpage = InstanceSubpage::LiveGameOutput(desired_output.clone());
        }

        // Every kind of content an instance can have gets its tab from the start, rather than
        // appearing once something of that kind is installed. Modpacks live among the mods, and
        // plugins are for servers.
        let mut tabs = vec![
            InstanceSubpageType::Quickplay,
            InstanceSubpageType::Logs,
            InstanceSubpageType::Mods,
            InstanceSubpageType::ResourcePacks,
            InstanceSubpageType::Shaders,
            InstanceSubpageType::Datapacks,
            InstanceSubpageType::Settings,
        ];
        if show_live_game_output {
            tabs.push(InstanceSubpageType::LiveGameOutput);
        }
        let current = self.subpage.page_type();
        let selected_index = tabs.iter().position(|tab| *tab == current).unwrap_or(0);

        v_flex()
            .size_full()
            .child(
                TabBar::new("bar")
                    .prefix(div().w_4())
                    .selected_index(selected_index)
                    .underline()
                    .children(tabs.iter().map(|tab| Tab::new().label(tab.label())))
                    .on_click(cx.listener(move |_, index: &usize, _, cx| {
                        if let Some(page_type) = tabs.get(*index).copied() {
                            InterfaceConfig::get_mut(cx).instance_subpage = page_type;
                        }
                    })),
            )
            .child(self.subpage.clone().into_any_element())
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceSubpageType {
    #[default]
    Quickplay,
    Logs,
    Mods,
    ResourcePacks,
    Shaders,
    Datapacks,
    Settings,
    LiveGameOutput,
}

impl InstanceSubpageType {
    fn label(self) -> &'static str {
        match self {
            InstanceSubpageType::Quickplay => t::instance::quickplay(),
            InstanceSubpageType::Logs => t::instance::logs::title(),
            InstanceSubpageType::Mods => t::instance::content::mods(),
            InstanceSubpageType::ResourcePacks => t::instance::content::resourcepacks(),
            InstanceSubpageType::Shaders => t::instance::content::shaders(),
            InstanceSubpageType::Datapacks => t::instance::content::datapacks(),
            InstanceSubpageType::Settings => t::settings::title(),
            InstanceSubpageType::LiveGameOutput => t::instance::live_game_output(),
        }
    }
}

impl InstanceSubpageType {
    pub fn create(
        self,
        instance: &Entity<InstanceEntry>,
        data: &DataEntities,
        backend_handle: BackendHandle,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> Option<InstanceSubpage> {
        Some(match self {
            InstanceSubpageType::Quickplay => {
                InstanceSubpage::Quickplay(cx.new(|cx| InstanceQuickplaySubpage::new(instance, data, window, cx)))
            },
            InstanceSubpageType::Logs => {
                InstanceSubpage::Logs(cx.new(|cx| InstanceLogsSubpage::new(instance, backend_handle, window, cx)))
            },
            InstanceSubpageType::Mods => {
                InstanceSubpage::Mods(cx.new(|cx| {
                    InstanceContentSubpage::new(instance, ContentType::Mods, data, backend_handle, window, cx)
                }))
            },
            InstanceSubpageType::ResourcePacks => InstanceSubpage::ResourcePacks(cx.new(|cx| {
                InstanceContentSubpage::new(instance, ContentType::ResourcePacks, data, backend_handle, window, cx)
            })),
            InstanceSubpageType::Shaders => InstanceSubpage::Shaders(cx.new(|cx| {
                InstanceContentSubpage::new(instance, ContentType::Shaders, data, backend_handle, window, cx)
            })),
            InstanceSubpageType::Datapacks => InstanceSubpage::Datapacks(cx.new(|cx| {
                InstanceContentSubpage::new(instance, ContentType::Datapacks, data, backend_handle, window, cx)
            })),
            InstanceSubpageType::Settings => InstanceSubpage::Settings(
                cx.new(|cx| InstanceSettingsSubpage::new(instance, data, backend_handle, window, cx)),
            ),
            InstanceSubpageType::LiveGameOutput => {
                if let Some(game_output) = instance.read(cx).live_game_output.clone() {
                    InstanceSubpage::LiveGameOutput(game_output)
                } else {
                    return None;
                }
            },
        })
    }
}

#[derive(Clone)]
pub enum InstanceSubpage {
    Quickplay(Entity<InstanceQuickplaySubpage>),
    Logs(Entity<InstanceLogsSubpage>),
    Mods(Entity<InstanceContentSubpage>),
    ResourcePacks(Entity<InstanceContentSubpage>),
    Shaders(Entity<InstanceContentSubpage>),
    Datapacks(Entity<InstanceContentSubpage>),
    Settings(Entity<InstanceSettingsSubpage>),
    LiveGameOutput(Entity<GameOutputRoot>),
}

impl InstanceSubpage {
    pub fn page_type(&self) -> InstanceSubpageType {
        match self {
            InstanceSubpage::Quickplay(_) => InstanceSubpageType::Quickplay,
            InstanceSubpage::Logs(_) => InstanceSubpageType::Logs,
            InstanceSubpage::Mods(_) => InstanceSubpageType::Mods,
            InstanceSubpage::ResourcePacks(_) => InstanceSubpageType::ResourcePacks,
            InstanceSubpage::Shaders(_) => InstanceSubpageType::Shaders,
            InstanceSubpage::Datapacks(_) => InstanceSubpageType::Datapacks,
            InstanceSubpage::Settings(_) => InstanceSubpageType::Settings,
            InstanceSubpage::LiveGameOutput(_) => InstanceSubpageType::LiveGameOutput,
        }
    }

    pub fn into_any_element(self) -> AnyElement {
        match self {
            Self::Quickplay(entity) => entity.into_any_element(),
            Self::Logs(entity) => entity.into_any_element(),
            Self::Mods(entity) => entity.into_any_element(),
            Self::ResourcePacks(entity) => entity.into_any_element(),
            Self::Shaders(entity) => entity.into_any_element(),
            Self::Datapacks(entity) => entity.into_any_element(),
            Self::Settings(entity) => entity.into_any_element(),
            Self::LiveGameOutput(entity) => entity.into_any_element(),
        }
    }
}
