#![deny(unused_must_use)]

use std::{
    collections::HashMap, io::Write, path::{Path, PathBuf}, sync::{Arc, RwLock}
};

use bridge::{
    handle::{BackendHandle, FrontendHandle, FrontendReceiver},
    message::{BridgeNotificationType, MessageToFrontend},
};
use gpui::*;
use gpui_component::{
    notification::{Notification, NotificationType}, Root, StyledExt, WindowExt
};
use indexmap::IndexMap;
use rand::RngCore;
use serde::Deserialize;
use tokio::sync::mpsc::Receiver;

use crate::{
    entity::{
        account::AccountEntries, instance::InstanceEntries, metadata::FrontendMetadata, DataEntities
    }, game_output::{GameOutput, GameOutputRoot}, interface_config::InterfaceConfig, processor::Processor, root::{LauncherRoot, LauncherRootGlobal}
};

pub mod component;
pub mod entity;
pub mod game_output;
pub mod modals;
pub mod pages;
pub mod interface_config;
pub mod png_render_cache;
pub mod processor;
pub mod root;
pub mod ui;

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
        Ok(Self::iter().filter_map(|p| p.starts_with(path).then(|| p.into())).collect())
    }
}

#[cfg(windows)]
pub const MAIN_FONT: &'static str = "Inter 24pt 24pt";
#[cfg(not(windows))]
pub const MAIN_FONT: &'static str = "Inter 24pt";

pub fn start(
    launcher_dir: PathBuf,
    panic_message: Arc<RwLock<Option<String>>>,
    deadlock_message: Arc<RwLock<Option<String>>>,
    backend_handle: BackendHandle,
    mut recv: FrontendReceiver,
) {
    let http_client = std::sync::Arc::new(
        reqwest_client::ReqwestClient::user_agent(
            "PandoraLauncher/0.1.0 (https://github.com/Moulberry/PandoraLauncher)",
        )
        .unwrap(),
    );

    Application::new().with_http_client(http_client).with_assets(Assets).run(move |cx: &mut App| {
        let _ = cx.text_system().add_fonts(vec![
            Assets.load("fonts/inter/Inter-Regular.ttf").unwrap().unwrap(),
            Assets.load("fonts/roboto-mono/RobotoMono-Regular.ttf").unwrap().unwrap(),
        ]);

        gpui_component::init(cx);
        InterfaceConfig::init(cx, launcher_dir.join("interface.json").into());

        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);

        let theme_folder = launcher_dir.join("themes");

        _ = gpui_component::ThemeRegistry::watch_dir(theme_folder.clone(), cx, move |cx| {
            let theme_name = InterfaceConfig::get(cx).active_theme.clone();
            if theme_name.is_empty() {
                return;
            }

            let Some(theme) = gpui_component::ThemeRegistry::global(cx).themes().get(&SharedString::new(theme_name.trim_ascii())).cloned() else {
                return;
            };

            gpui_component::Theme::global_mut(cx).apply_config(&theme);
        });

        let theme = gpui_component::Theme::global_mut(cx);
        theme.font_family = SharedString::new_static(MAIN_FONT);
        theme.scrollbar_show = gpui_component::scroll::ScrollbarShow::Always;

        cx.on_app_quit(|cx| {
            InterfaceConfig::force_save(cx);
            async {}
        }).detach();

        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        }).detach();

        cx.open_window(
            WindowOptions {
                app_id: Some("PandoraLauncher".into()),
                window_min_size: Some(size(px(360.0), px(240.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::new_static("Pandora")),
                    ..Default::default()
                }),
                window_decorations: Some(WindowDecorations::Server),
                ..Default::default()
            },
            |window, cx| {
                let instances = cx.new(|_| InstanceEntries {
                    entries: IndexMap::new(),
                });
                let metadata = cx.new(|_| FrontendMetadata::new(backend_handle.clone()));
                let accounts = cx.new(|_| AccountEntries::default());
                let data = DataEntities {
                    instances,
                    metadata,
                    backend_handle,
                    accounts,
                    theme_folder: theme_folder.into(),
                };

                let mut processor = Processor::new(data.clone(), window.window_handle());

                while let Some(message) = recv.try_recv() {
                    processor.process(message, cx);
                }

                cx.spawn(async move |cx| {
                    while let Some(message) = recv.recv().await {
                        _ = cx.update(|cx| {
                            processor.process(message, cx);
                        });
                    }
                }).detach();

                window.set_window_title("Pandora");

                let launcher_root = cx.new(|cx| LauncherRoot::new(&data, panic_message, deadlock_message, window, cx));
                cx.set_global(LauncherRootGlobal {
                    root: launcher_root.clone(),
                });
                cx.new(|cx| Root::new(launcher_root, window, cx))
            },
        ).unwrap();

        cx.activate(true);
    });
}

pub(crate) fn is_valid_instance_name(name: &str) -> bool {
    is_single_component_path(name) &&
    sanitize_filename::is_sanitized_with_options(name, sanitize_filename::OptionsForCheck { windows: true, ..Default::default() })
}

pub(crate) fn is_single_component_path(path: &str) -> bool {
    let path = std::path::Path::new(path);
    let mut components = path.components().peekable();

    if let Some(first) = components.peek()
        && !matches!(first, std::path::Component::Normal(_))
    {
        return false;
    }

    components.count() == 1
}

#[inline]
pub(crate) fn labelled(label: &'static str, element: impl IntoElement) -> Div {
    gpui_component::v_flex().gap_0p5().child(div().text_sm().font_medium().child(label)).child(element)
}

pub(crate) fn open_folder(path: &Path, window: &mut Window, cx: &mut App) {
    if path.is_dir() {
        if let Err(err) = open::that_detached(path) {
            let notification: Notification = (NotificationType::Error, SharedString::from(format!("Unable to open folder: {err}"))).into();
            window.push_notification(notification.autohide(false), cx);
        }
    } else {
        let notification: Notification = (NotificationType::Error, SharedString::from("Unable to open folder: not a directory")).into();
        window.push_notification(notification.autohide(false), cx);
    }
}
