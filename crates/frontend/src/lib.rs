#![deny(unused_must_use)]

use std::{collections::HashMap, sync::{Arc, RwLock}};

use bridge::{handle::BackendHandle, message::{BridgeNotificationType, MessageToFrontend}};
use gpui::{http_client::HttpClient, prelude::*, *};
use gpui_component::{notification::{Notification, NotificationType}, Root, ThemeMode, WindowExt};
use indexmap::IndexMap;
use tokio::sync::mpsc::Receiver;

use crate::{entity::{account::AccountEntries, instance::InstanceEntries, modrinth::FrontendModrinthData, version::VersionEntries, DataEntities}, game_output::{GameOutput, GameOutputRoot}, root::{LauncherRoot, LauncherRootGlobal}};

pub mod root;
pub mod ui;
pub mod entity;
pub mod component;
pub mod pages;
pub mod game_output;
pub mod modals;
pub mod png_render_cache;

rust_i18n::i18n!("locales");

macro_rules! ts {
    ($($all:tt)*) => {
        SharedString::new_static(ustr::ustr(&*rust_i18n::t!($($all)*)).as_str())
    };
}
pub(crate) use ts;

#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets"]
#[include = "icons/**/*.svg"]
#[include = "images/**/*.png"]
#[include = "fonts/**/*.ttf"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        Self::get(path)
            .map(|f| Some(f.data))
            .ok_or_else(|| anyhow::anyhow!("could not find asset at path \"{path}\""))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect())
    }
}

pub fn start(panic_message: Arc<RwLock<Option<String>>>, backend_handle: BackendHandle, mut recv: Receiver<MessageToFrontend>) {
    let http_client = std::sync::Arc::new(
        reqwest_client::ReqwestClient::user_agent("PandoraLauncher/0.1.0 (https://github.com/Moulberry/PandoraLauncher)").unwrap(),
    );
    
    Application::new().with_http_client(http_client).with_assets(Assets).run(|cx: &mut App| {
        let _ = cx.text_system()
            .add_fonts(vec![
                Assets.load("fonts/inter/Inter-Regular.ttf").unwrap().unwrap(),
                Assets.load("fonts/roboto-mono/RobotoMono-Regular.ttf").unwrap().unwrap()
            ]);
        
        gpui_component::init(cx);
        gpui_component::Theme::change(ThemeMode::Dark, None, cx);
        
        let theme = gpui_component::Theme::global_mut(cx);
        theme.font_family = SharedString::new_static("Inter 24pt");
        theme.scrollbar_show = gpui_component::scroll::ScrollbarShow::Always;

        let bounds = Bounds::centered(None, size(px(500.), px(500.0)), cx);

        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        }).detach();
        
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(300.0), px(200.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::new_static("Pandora")),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let instances = cx.new(|_| InstanceEntries { entries: IndexMap::new() });
                let versions = cx.new(|_| VersionEntries::new(backend_handle.clone()));
                let modrinth = cx.new(|_| FrontendModrinthData::new(backend_handle.clone()));
                let accounts = cx.new(|_| AccountEntries::default());
                let data = DataEntities {
                    instances,
                    versions,
                    backend_handle,
                    modrinth,
                    accounts,
                };
                
                {
                    let data = data.clone();
                    let mut game_output_windows = HashMap::new();
                    let window_handle = window.window_handle();
                    cx.spawn(async move |cx| {
                        while let Some(message) = recv.recv().await {
                            match message {
                                MessageToFrontend::VersionManifestUpdated(result) => {
                                    VersionEntries::set(&data.versions, result, cx);
                                },
                                MessageToFrontend::AccountsUpdated { accounts, selected_account } => {
                                    AccountEntries::set(&data.accounts, accounts, selected_account, cx);
                                },
                                MessageToFrontend::InstanceAdded { id, name, version, loader, worlds_state, servers_state, mods_state } => {
                                    InstanceEntries::add(&data.instances, id, name.as_str().into(), version.as_str().into(), loader, worlds_state, servers_state, mods_state, cx);
                                },
                                MessageToFrontend::InstanceRemoved { id } => {
                                    InstanceEntries::remove(&data.instances, id, cx);
                                },
                                MessageToFrontend::InstanceModified { id, name, version, loader, status } => {
                                    InstanceEntries::modify(&data.instances, id, name.as_str().into(), version.as_str().into(), loader, status, cx);
                                },
                                MessageToFrontend::InstanceWorldsUpdated { id, worlds } => {
                                    InstanceEntries::set_worlds(&data.instances, id, worlds, cx);
                                },
                                MessageToFrontend::InstanceServersUpdated { id, servers } => {
                                    InstanceEntries::set_servers(&data.instances, id, servers, cx);
                                },
                                MessageToFrontend::InstanceModsUpdated { id, mods } => {
                                    InstanceEntries::set_mods(&data.instances, id, mods, cx);
                                },
                                MessageToFrontend::AddNotification { notification_type, message } => {
                                    window_handle.update(cx, |_, window, cx| {
                                        let notification_type = match notification_type {
                                            BridgeNotificationType::Success => NotificationType::Success,
                                            BridgeNotificationType::Info => NotificationType::Info,
                                            BridgeNotificationType::Error => NotificationType::Error,
                                            BridgeNotificationType::Warning => NotificationType::Warning,
                                        };
                                        let mut notification: Notification = (notification_type, SharedString::from(message)).into();
                                        if let NotificationType::Error = notification_type {
                                            notification = notification.autohide(false);
                                        }
                                        window.push_notification(notification, cx);
                                    }).unwrap();
                                },
                                MessageToFrontend::Refresh => {
                                    let _ = cx.refresh();
                                },
                                MessageToFrontend::CreateGameOutputWindow { id, keep_alive } => {
                                    _ = cx.open_window(WindowOptions::default(), |window, cx| {
                                        let game_output = cx.new(|_| GameOutput::default());
                                        let game_output_root = cx.new(|cx| {
                                            GameOutputRoot::new(keep_alive, game_output.clone(), window, cx)
                                        });
                                        window.activate_window();
                                        let window_handle = window.window_handle().downcast::<Root>().unwrap();
                                        game_output_windows.insert(id, (window_handle, game_output.clone()));
                                        cx.new(|cx| Root::new(game_output_root.into(), window, cx))
                                    });
                                },
                                MessageToFrontend::AddGameOutput { id, time, thread, level, text } => {
                                    if let Some((window, game_output)) = game_output_windows.get(&id) {
                                        _ = window.update(cx, |_, window, cx| {
                                            game_output.update(cx, |game_output, _| {
                                                game_output.add(time, thread, level, text);
                                            });
                                            window.refresh();
                                        });
                                    }
                                },
                                MessageToFrontend::ModrinthDataUpdated { request, result, alive_handle } => {
                                    FrontendModrinthData::set(&data.modrinth, request, result, alive_handle, cx);
                                },
                            }
                        }
                    }).detach();
                }
                
                window.set_window_title("Pandora");
                
                let launcher_root = cx.new(|cx| LauncherRoot::new(&data, panic_message, window, cx));
                cx.set_global(LauncherRootGlobal {
                    root: launcher_root.clone()
                });
                cx.new(|cx| Root::new(launcher_root.into(), window, cx))
            },
        ).unwrap();
        
        cx.activate(true);
    });
}

pub(crate) fn is_single_component_path(path: &str) -> bool {
    let path = std::path::Path::new(path);
    let mut components = path.components().peekable();

    if let Some(first) = components.peek() {
        if !matches!(first, std::path::Component::Normal(_)) {
            return false;
        }
    }

    components.count() == 1
}
