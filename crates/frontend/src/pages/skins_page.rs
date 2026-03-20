use std::{path::Path, sync::Arc};

use bridge::{
    handle::BackendHandle,
    message::{MessageToBackend, MinecraftProfileInfo},
    modal_action::ModalAction,
};
use gpui::{InteractiveElement, IntoElement, ParentElement, Styled, Window, prelude::*, *};
use gpui_component::{
    Disableable, IconName,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    h_flex,
    scroll::ScrollableElement,
    v_flex,
};

use crate::{
    component::{cape_card, skin_renderer::SkinRenderer},
    entity::{account::{AccountEntries, AccountChanged}, minecraft_profile::MinecraftProfileEntries, skin_thumbnail_cache::SkinThumbnailCache, DataEntities},
    interface_config::InterfaceConfig,
    modals::upload_skin_modal,
    pages::page::{Page, page_layout},
    ui::PageType,
};

/// OptiFine serves donor capes at this URL by Minecraft username (same as used by the Capes mod).
/// See: https://github.com/CaelTheColher/Capes and https://optifine.readthedocs.io/capes.html
fn optifine_cape_url(username: &str) -> String {
    format!("https://optifine.net/capes/{}.png", username)
}

enum SkinPageState {
    Loading,
    NotAuthenticated,
    Ready(MinecraftProfileInfo),
}

const INITIAL_SKINS_VISIBLE: usize = 8;

pub struct SkinsPage {
    backend_handle: BackendHandle,
    minecraft_profile: Entity<MinecraftProfileEntries>,
    accounts: Entity<AccountEntries>,
    skin_thumbnail_cache: Entity<SkinThumbnailCache>,
    launcher_dir: Arc<Path>,
    state: SkinPageState,
    selected_skin_id: Option<Arc<str>>,
    selected_cape_id: Option<uuid::Uuid>,
    skins_expanded: bool,
    optifine_cape_in_preview: bool,
    _subscription: Subscription,
    _account_subscription: Subscription,
    _get_profile_task: Task<()>,
    pub skin_renderer: Entity<SkinRenderer>,
    _download_active_skin_task: Option<Task<()>>,
    last_rendered_skin_url: Option<String>,
    _download_active_cape_task: Option<Task<()>>,
    last_rendered_cape_url: Option<String>,
    _download_optifine_cape_task: Option<Task<()>>,
    last_rendered_optifine_username: Option<String>,

    _thumbnail_tasks: std::collections::HashMap<Arc<str>, Task<()>>,
    _cape_thumbnail_tasks: std::collections::HashMap<Arc<str>, Task<()>>, // key: "skin_url\0cape_url"
}

impl SkinsPage {
    pub fn new(data: &DataEntities, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let profile = data.minecraft_profile.read(cx).profile.clone();

        let state = match profile {
            Some(p) => SkinPageState::Ready(p),
            None => SkinPageState::Loading,
        };

        let _subscription = cx.subscribe(&data.minecraft_profile, |this, _, _, cx| {
            this.refresh_from_entity(cx);
            this.update_skin_renderer(cx);
            cx.notify();
        });

        let _account_subscription = cx.subscribe::<_, AccountChanged>(&data.accounts, |this, _, _, cx| {
            this.load_profile(cx);
            this.update_skin_renderer(cx);
            cx.notify();
        });

        let mut page = Self {
            backend_handle: data.backend_handle.clone(),
            minecraft_profile: data.minecraft_profile.clone(),
            accounts: data.accounts.clone(),
            skin_thumbnail_cache: data.skin_thumbnail_cache.clone(),
            launcher_dir: data.launcher_dir.clone(),
            state,
            selected_skin_id: None,
            selected_cape_id: None,
            skins_expanded: false,
            optifine_cape_in_preview: false,
            _subscription,
            _account_subscription,
            _get_profile_task: Task::ready(()),
            skin_renderer: cx.new(|_| SkinRenderer::new(None, false)),
            _download_active_skin_task: None,
            last_rendered_skin_url: None,
            _download_active_cape_task: None,
            last_rendered_cape_url: None,
            _download_optifine_cape_task: None,
            last_rendered_optifine_username: None,
            _thumbnail_tasks: std::collections::HashMap::new(),
            _cape_thumbnail_tasks: std::collections::HashMap::new(),
        };
        page.load_profile(cx);
        page.update_skin_renderer(cx);
        page
    }

    fn active_skin_id(profile: &MinecraftProfileInfo) -> Option<Arc<str>> {
        profile.skins.iter().find(|s| s.state.as_ref() == "ACTIVE").map(|s| s.id.clone())
    }

    fn active_cape_id(profile: &MinecraftProfileInfo) -> Option<uuid::Uuid> {
        profile
            .capes
            .iter()
            .find(|c| c.state.as_ref() == "ACTIVE")
            .and_then(|c| uuid::Uuid::parse_str(c.id.as_ref()).ok())
    }

    fn reset_pending_selection(&mut self, profile: &MinecraftProfileInfo, cx: &mut Context<Self>) {
        self.selected_skin_id = Self::active_skin_id(profile);
        self.selected_cape_id = Self::active_cape_id(profile);
        self.update_skin_renderer(cx);
    }

    fn apply_pending_selection(&mut self, profile: &MinecraftProfileInfo) {
        let active_skin_id = Self::active_skin_id(profile);
        if self.selected_skin_id != active_skin_id
            && let Some(selected_skin_id) = self.selected_skin_id.as_ref()
            && let Some(selected_skin) = profile.skins.iter().find(|skin| &skin.id == selected_skin_id)
        {
            if let Some(path) = &selected_skin.local_path {
                self.set_skin_from_path(path.clone(), selected_skin.variant.clone());
            } else {
                self.set_skin(selected_skin.url.clone(), selected_skin.variant.clone());
            }
        }

        let active_cape_id = Self::active_cape_id(profile);
        if self.selected_cape_id != active_cape_id {
            self.set_cape(self.selected_cape_id);
        }
    }

    fn refresh_from_entity(&mut self, cx: &mut Context<Self>) {
        let profile = self.minecraft_profile.read(cx).profile.clone();
        self.state = match profile {
            Some(p) => {
                self.selected_skin_id = Self::active_skin_id(&p);
                self.selected_cape_id = Self::active_cape_id(&p);
                SkinPageState::Ready(p)
            },
            None => SkinPageState::NotAuthenticated,
        };
    }

    fn load_profile(&mut self, cx: &mut Context<Self>) {
        if self.accounts.read(cx).selected_account.is_none() {
            self.state = SkinPageState::NotAuthenticated;
            return;
        }

        if let SkinPageState::Ready(_) = self.state {
            // Already have data, don't show loading screen
        } else {
            self.state = SkinPageState::Loading;
        }

        let action = ModalAction::default();
        let action_clone = action.clone();
        let profile_entity = self.minecraft_profile.clone();

        self.backend_handle.send(MessageToBackend::GetMinecraftProfile { modal_action: action });

        self._get_profile_task = cx.spawn(async move |this, cx| {
            let mut elapsed_ms = 0;
            while action_clone.get_finished_at().is_none() && elapsed_ms < 10000 {
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                elapsed_ms += 100;
            }

            let _ = this.update(cx, |this, cx| {
                if let SkinPageState::Loading = this.state {
                    let profile_result = profile_entity.read(cx).profile.clone();
                    if let Some(p) = profile_result {
                        this.selected_skin_id = Self::active_skin_id(&p);
                        this.selected_cape_id = Self::active_cape_id(&p);
                        this.state = SkinPageState::Ready(p);
                    } else {
                        this.state = SkinPageState::NotAuthenticated;
                    }
                    this.update_skin_renderer(cx);
                    cx.notify();
                }
            });
        });
    }

    fn set_skin(&mut self, url: Arc<str>, variant: Arc<str>) {
        self.backend_handle.send(MessageToBackend::SetSkin {
            skin_url: url,
            skin_variant: variant,
            modal_action: ModalAction::default(),
        });
    }

    fn set_skin_from_path(&mut self, path: Arc<str>, variant: Arc<str>) {
        self.backend_handle.send(MessageToBackend::SetSkinFromPath {
            path,
            skin_variant: variant,
            modal_action: ModalAction::default(),
        });
    }

    fn set_cape(&mut self, cape_id: Option<uuid::Uuid>) {
        self.backend_handle.send(MessageToBackend::SetCape {
            cape_id,
            modal_action: ModalAction::default(),
        });
    }

    fn update_skin_renderer(&mut self, cx: &mut Context<Self>) {
        if let SkinPageState::Ready(profile) = &self.state {
            // Update nameplate with profile name
            let profile_name = profile.name.clone();
            self.skin_renderer.update(cx, |r, _| {
                r.nameplate = Some(profile_name.clone().into());
            });

            let preview_skin = self
                .selected_skin_id
                .as_ref()
                .and_then(|selected_skin_id| profile.skins.iter().find(|skin| &skin.id == selected_skin_id))
                .or_else(|| profile.skins.iter().find(|s| s.state.as_ref() == "ACTIVE"));

            if let Some(preview_skin) = preview_skin {
                let render_source = preview_skin
                    .local_path
                    .as_ref()
                    .map(|path| path.to_string())
                    .unwrap_or_else(|| preview_skin.url.to_string());
                let is_slim = preview_skin.variant.as_ref() == "SLIM";
                if self.last_rendered_skin_url.as_deref() != Some(render_source.as_str()) {
                    self.last_rendered_skin_url = Some(render_source.clone());
                    let skin_renderer = self.skin_renderer.clone();
                    let client = cx.http_client();
                    let preview_skin_local_path = preview_skin.local_path.clone();
                    let preview_skin_url = preview_skin.url.clone();
                    self._download_active_skin_task = Some(cx.spawn(async move |_page, cx| {
                        let bytes: Option<Arc<[u8]>> = if let Some(local_path) = preview_skin_local_path {
                            std::fs::read(local_path.as_ref()).ok().map(|bytes| Arc::from(bytes.into_boxed_slice()))
                        } else if let Ok(mut response) = client.get(preview_skin_url.as_ref(), ().into(), true).await {
                            use futures::AsyncReadExt;
                            let mut bytes = Vec::new();
                            if response.body_mut().read_to_end(&mut bytes).await.is_ok() {
                                Some(Arc::from(bytes.into_boxed_slice()))
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        if let Some(data) = bytes {
                            let _ = skin_renderer.update(cx, |r, _| r.update_image(Some(data), is_slim));
                        }
                    }));
                } else {
                    self.skin_renderer.update(cx, |r, _| r.slim = is_slim);
                }
            } else {
                self.skin_renderer.update(cx, |r, _| r.update_image(None, false));
            }

            // Cape logic: OptiFine toggle takes precedence when ON. When OFF, show Mojang cape if equipped.
            if self.optifine_cape_in_preview {
                // Show OptiFine cape when toggle is on (same API as Capes mod)
                self.last_rendered_cape_url = None;
                let username = profile.name.to_string();
                if self.last_rendered_optifine_username.as_deref() != Some(username.as_str()) {
                    let optifine_url = optifine_cape_url(&username);
                    let skin_renderer = self.skin_renderer.clone();
                    let client = cx.http_client();
                    let username_clone = username.clone();
                    self._download_optifine_cape_task = Some(cx.spawn(async move |this, cx| {
                        let ok = async {
                            let mut response = client.get(&optifine_url, ().into(), true).await.ok()?;
                            // OptiFine returns 404 if the player has no donor cape
                            if response.status().as_u16() != 200 {
                                return None;
                            }
                            use futures::AsyncReadExt;
                            let mut bytes = Vec::new();
                            response.body_mut().read_to_end(&mut bytes).await.ok()?;
                            Some(Arc::from(bytes.into_boxed_slice()) as Arc<[u8]>)
                        };
                        if let Some(data) = ok.await {
                            let _ = skin_renderer.update(cx, |r, cx| {
                                r.update_cape(Some(data));
                                cx.notify();
                            });
                        } else {
                            let _ = skin_renderer.update(cx, |r, cx| {
                                r.update_cape(None);
                                cx.notify();
                            });
                        }
                        let _ = this.update(cx, |p, cx| {
                            p.last_rendered_optifine_username = Some(username_clone);
                            cx.notify();
                        });
                    }));
                }
            } else if let Some(selected_cape_id) = self.selected_cape_id
                && let Some(selected_cape) = profile
                    .capes
                    .iter()
                    .find(|cape| uuid::Uuid::parse_str(cape.id.as_ref()).ok() == Some(selected_cape_id))
            {
                self.last_rendered_optifine_username = None;
                let url = selected_cape.url.to_string();
                if self.last_rendered_cape_url.as_deref() != Some(url.as_str()) {
                    self.last_rendered_cape_url = Some(url.clone());
                    let skin_renderer = self.skin_renderer.clone();
                    let client = cx.http_client();
                    self._download_active_cape_task = Some(cx.spawn(async move |_page, cx| {
                        if let Ok(mut response) = client.get(&url, ().into(), true).await {
                            use futures::AsyncReadExt;
                            let mut bytes = Vec::new();
                            if response.body_mut().read_to_end(&mut bytes).await.is_ok() {
                                let data: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());
                                let _ = skin_renderer.update(cx, |r, cx| {
                                    r.update_cape(Some(data));
                                    cx.notify();
                                });
                            }
                        }
                    }));
                }
            } else {
                self.last_rendered_cape_url = None;
                if self.last_rendered_optifine_username.take().is_some() {
                    self.skin_renderer.update(cx, |r, cx| {
                        r.update_cape(None);
                        cx.notify();
                    });
                }
            }

            // Lazy load thumbnails - keyed by URL to avoid duplicates
            let mut urls_to_load: Vec<(String, Arc<str>, bool)> = Vec::new();
            let thumbnail_cache = self.skin_thumbnail_cache.read(cx);
            for skin in &profile.skins {
                let is_slim = skin.variant.as_ref() == "SLIM";
                
                // Use local_path if available, otherwise use URL
                let url_key = skin.url.to_string();
                let load_url = if let Some(local) = &skin.local_path {
                    format!("file://{}", local)
                } else {
                    url_key.clone()
                };
                
                // Skip if already cached or loading
                let url_key_arc: Arc<str> = url_key.clone().into();
                if !thumbnail_cache.contains(url_key.as_str()) && !self._thumbnail_tasks.contains_key(&url_key_arc) {
                    urls_to_load.push((load_url.clone(), url_key_arc.clone(), is_slim));
                }
            }
            let _ = thumbnail_cache;

            // Load thumbnails
            for (load_url, cache_key, is_slim) in urls_to_load {
                let client = cx.http_client();
                let cache_key_clone = cache_key.clone();
                let task = cx.spawn(async move |this, cx| {
                    let bytes: Option<Arc<[u8]>> = if load_url.starts_with("file://") {
                        // Load from local file
                        let path = &load_url[7..]; // Remove "file://" prefix
                        std::fs::read(path).ok().map(|b| Arc::from(b.into_boxed_slice()))
                    } else {
                        // Load from URL
                        if let Ok(mut response) = client.get(&load_url, ().into(), true).await {
                            use futures::AsyncReadExt;
                            let mut bytes = Vec::new();
                            if response.body_mut().read_to_end(&mut bytes).await.is_ok() {
                                Some(Arc::from(bytes.into_boxed_slice()))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    };
                    
                    if let Some(data) = bytes {
                        let renderer = SkinRenderer::new(Some(data), is_slim);
                        let front = renderer.render_to_buffer_with_params(200, 200, 0.3, 0.05, true);
                        let back = renderer.render_to_buffer_with_params(200, 200, std::f32::consts::PI + 0.3, 0.05, true);
                        // Capeless back with same framing as cape thumbnails (for None card)
                        let back_yaw = std::f32::consts::PI + 0.3;
                        let none_card_back = renderer.render_to_buffer_with_params_ext(200, 200, back_yaw, 0.05, true, true);

                        if let (Some(f), Some(b)) = (front, back) {
                            let _ = this.update(cx, |this, cx| {
                                this.skin_thumbnail_cache.update(cx, |cache, _| {
                                    cache.insert(cache_key_clone.clone(), f, b);
                                    if let Some(nb) = none_card_back {
                                        cache.insert_none_card(cache_key_clone.clone(), nb);
                                    }
                                });
                                cx.notify();
                            });
                        }
                    }
                });
                self._thumbnail_tasks.insert(cache_key, task);
            }

            // Lazy load cape thumbnails (render 3D model with active skin + each cape)
            if let Some(active_skin) = profile.skins.iter().find(|s| s.state.as_ref() == "ACTIVE") {
                let skin_url: String = active_skin.url.to_string();
                let skin_url_arc: Arc<str> = skin_url.clone().into();
                let skin_load_url = if let Some(local) = &active_skin.local_path {
                    format!("file://{}", local)
                } else {
                    skin_url.clone()
                };
                let is_slim = active_skin.variant.as_ref() == "SLIM";
                let thumbnail_cache = self.skin_thumbnail_cache.read(cx);
                for cape in &profile.capes {
                    let cape_url: String = cape.url.to_string();
                    let cache_key: Arc<str> = format!("{}\0{}", skin_url, cape_url).into();
                    if !thumbnail_cache.contains_cape(&skin_url, &cape_url)
                        && !self._cape_thumbnail_tasks.contains_key(&cache_key)
                    {
                        let client = cx.http_client();
                        let skin_load_url = skin_load_url.clone();
                        let cape_url_clone = cape_url.clone();
                        let skin_url_for_insert = skin_url_arc.clone();
                        let task = cx.spawn(async move |this, cx| {
                            use futures::AsyncReadExt;
                            let skin_bytes: Option<Arc<[u8]>> = if skin_load_url.starts_with("file://") {
                                std::fs::read(&skin_load_url[7..]).ok().map(|b| Arc::from(b.into_boxed_slice()))
                            } else if let Ok(mut r) = client.get(&skin_load_url, ().into(), true).await {
                                let mut bytes = Vec::new();
                                if r.body_mut().read_to_end(&mut bytes).await.is_ok() {
                                    Some(Arc::from(bytes.into_boxed_slice()))
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            let cape_bytes: Option<Arc<[u8]>> = if let Ok(mut r) = client.get(&cape_url_clone, ().into(), true).await {
                                let mut bytes = Vec::new();
                                if r.body_mut().read_to_end(&mut bytes).await.is_ok() {
                                    Some(Arc::from(bytes.into_boxed_slice()))
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            if let (Some(skin), Some(cape)) = (skin_bytes, cape_bytes) {
                                let mut renderer = SkinRenderer::new(Some(skin), is_slim);
                                renderer.update_cape(Some(cape));
                                // Cape cards show back view only; use wider framing to fit whole player + full cape
                                let back_yaw = std::f32::consts::PI + 0.3;
                                let front = renderer.render_to_buffer_with_params_ext(200, 200, 0.3, 0.05, true, true);
                                let back = renderer.render_to_buffer_with_params_ext(200, 200, back_yaw, 0.05, true, true);
                                if let (Some(f), Some(b)) = (front, back) {
                                    let _ = this.update(cx, |this, cx| {
                                        this.skin_thumbnail_cache.update(cx, |c, _| {
                                            c.insert_cape(
                                                skin_url_for_insert.clone(),
                                                cape_url_clone.clone().into(),
                                                f,
                                                b,
                                            );
                                        });
                                        cx.notify();
                                    });
                                }
                            }
                        });
                        self._cape_thumbnail_tasks.insert(cache_key, task);
                    }
                }
                let _ = thumbnail_cache;
            }
        }
    }
}

impl Page for SkinsPage {
    fn controls(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let SkinPageState::Ready(profile) = &self.state else {
            return gpui::Empty.into_any_element();
        };

        let active_skin_id = Self::active_skin_id(profile);
        let active_cape_id = Self::active_cape_id(profile);
        let can_apply_changes =
            self.selected_skin_id != active_skin_id || self.selected_cape_id != active_cape_id;
        let profile = profile.clone();
        let account_name: String = (&*profile.name).to_string();
        let skins_folder = self.launcher_dir.join("owned_skins").join(&account_name);

        h_flex()
            .gap_2()
            .child(
                Button::new("reset_skin_changes")
                    .label("Reset")
                    .disabled(!can_apply_changes)
                    .on_click({
                        let profile = profile.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.reset_pending_selection(&profile, cx);
                            cx.notify();
                        })
                    }),
            )
            .child(
                Button::new("apply_skin_changes")
                    .label("Apply Changes")
                    .success()
                    .disabled(!can_apply_changes)
                    .on_click({
                        let profile = profile.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.apply_pending_selection(&profile);
                            this.load_profile(cx);
                            cx.notify();
                        })
                    }),
            )
            .child(
                Button::new("open_skins_folder")
                    .ml_2()
                    .icon(IconName::FolderOpen)
                    .label("Open skins folder")
                    .on_click(move |_button, window, cx| {
                        crate::open_folder(&skins_folder, window, cx);
                    }),
            )
            .into_any_element()
    }

    fn scrollable(&self, _cx: &App) -> bool {
        true
    }
}

impl Render for SkinsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = v_flex().p_4().gap_4().children(match &self.state {
            SkinPageState::Loading => {
                vec![div().child("Loading...").into_any_element()]
            },
            SkinPageState::NotAuthenticated => {
                vec![
                    div().child("Not Authenticated").into_any_element(),
                    div().child("Please log in with a Minecraft account to manage skins.").into_any_element(),
                ]
            },
            SkinPageState::Ready(profile) => {
                let optifine_cape_in_preview = self.optifine_cape_in_preview;
                let active_cape_id = Self::active_cape_id(profile);
                let left_panel = v_flex()
                    .w(px(320.0))
                    .flex_shrink_0()
                    .p_4()
                    .bg(gpui::rgba(0x1e1e24ff))
                    .rounded_lg()
                    .child(div().text_xl().font_weight(FontWeight::BOLD).child("3D Preview"))
                    .child(
                        div()
                            .mt_4()
                            .text_color(gpui::rgba(0xccccccff))
                            .child("(Drag to rotate)"),
                    )
                    .child(
                        div()
                            .w(px(288.0))
                            .h(px(288.0))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .child(self.skin_renderer.clone()),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .mt_4()
                            .pt_4()
                            .border_t_1()
                            .border_color(gpui::rgba(0x404050ff))
                            .child(
                                Checkbox::new("optifine_cape_preview")
                                    .label("Show OptiFine cape in preview")
                                    .checked(optifine_cape_in_preview)
                                    .on_click(cx.listener(move |this, value, _, cx| {
                                        this.optifine_cape_in_preview = *value;
                                        this.last_rendered_optifine_username = None;
                                        this.update_skin_renderer(cx);
                                        cx.notify();
                                    })),
                            ),
                    );

                let add_skin_card = div()
                    .flex()
                    .flex_col()
                    .w(px(155.0))
                    .h(px(155.0))
                    .bg(gpui::rgba(0x2d2d35ff))
                    .rounded_lg()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                        upload_skin_modal::open(this.backend_handle.clone(), window, cx);
                    }))
                    .child(div().text_3xl().child("+"))
                    .child(div().text_sm().child("Add a skin"));

                let mut skin_cards = Vec::new();
                skin_cards.push(add_skin_card.into_any_element());
                
                for skin in profile.skins.iter().filter(|skin| {
                    // Skip local skins whose file was deleted (card should disappear immediately)
                    if let Some(local) = &skin.local_path {
                        let path = Path::new(&**local);
                        if !path.exists() {
                            return false;
                        }
                    }
                    true
                }) {
                    let is_equipped = skin.state.as_ref() == "ACTIVE";
                    let is_selected = self.selected_skin_id.as_ref() == Some(&skin.id);
                    let thumbnail_cache = self.skin_thumbnail_cache.read(cx);
                    let (front, back) = thumbnail_cache.get(&skin.url).map(|(f, b)| (Some(f.clone()), Some(b.clone()))).unwrap_or((None, None));
                    let _ = thumbnail_cache;
                    let skin_id = skin.id.clone();
                    let this_entity = cx.entity().clone();
                    
                    // Get file path for reveal (as string)
                    let skin_file_path_str = if let Some(local) = &skin.local_path {
                        let local_str: String = (&*local).to_string();
                        let path = Path::new(&local_str);
                        if path.exists() {
                            Some(local_str)
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    skin_cards.push(
                        crate::component::skin_card::render_skin_card(
                            skin.id.clone(),
                            is_selected,
                            is_equipped,
                            skin.url.clone(),
                            skin.variant.clone(),
                            front,
                            back,
                            move |_, cx| {
                                let _ = this_entity.update(cx, |this, cx| {
                                    this.selected_skin_id = Some(skin_id.clone());
                                    this.update_skin_renderer(cx);
                                    cx.notify();
                                });
                            },
                            skin_file_path_str.map(|file_path| {
                                let fp = file_path.clone();
                                move |_: &mut Window, _: &mut App| {
                                    let path = Path::new(&fp).to_path_buf();
                                    std::thread::spawn(move || {
                                        #[cfg(target_os = "macos")]
                                        {
                                            use std::process::Command;
                                            let _ = Command::new("open").args(["-R", &path.to_string_lossy()]).spawn();
                                        }
                                        #[cfg(target_os = "windows")]
                                        {
                                            use std::process::Command;
                                            let _ = Command::new("explorer").args(["/select,", &path.to_string_lossy()]).spawn();
                                        }
                                        #[cfg(target_os = "linux")]
                                        {
                                            use std::process::Command;
                                            let _ = Command::new("xdg-open").arg(path.parent().unwrap_or(&path)).spawn();
                                        }
                                    });
                                }
                            }),
                        ).into_any_element()
                    );
                }

                let has_active_cape = active_cape_id.is_some();
                let active_skin_url = profile
                    .skins
                    .iter()
                    .find(|s| s.state.as_ref() == "ACTIVE")
                    .map(|s| s.url.to_string())
                    .unwrap_or_default();
                let thumbnail_cache = self.skin_thumbnail_cache.read(cx);
                // None card: capeless skin backside with same framing as cape thumbnails
                let none_card_back = thumbnail_cache.get_none_card(&active_skin_url);
                let mut cape_cards: Vec<_> = Vec::new();
                let this_entity = cx.entity().clone();
                cape_cards.push(
                    cape_card::render_cape_card(
                        "none".into(),
                        self.selected_cape_id.is_none(),
                        !has_active_cape,
                        None,
                        none_card_back,
                        "None",
                        move |_, cx| {
                            let _ = this_entity.update(cx, |this, cx| {
                                this.selected_cape_id = None;
                                this.update_skin_renderer(cx);
                                cx.notify();
                            });
                        },
                    ).into_any_element(),
                );
                for cape in &profile.capes {
                    let cape_uuid = uuid::Uuid::parse_str(cape.id.as_ref()).ok();
                    let is_equipped = cape.state.as_ref() == "ACTIVE";
                    let is_selected = cape_uuid.is_some() && self.selected_cape_id == cape_uuid;
                    let (front, back) = thumbnail_cache
                        .get_cape(&active_skin_url, &*cape.url)
                        .map(|(f, b)| (Some(f.clone()), Some(b.clone())))
                        .unwrap_or((None, None));
                    let this_entity = cx.entity().clone();
                    cape_cards.push(
                        cape_card::render_cape_card(
                            cape.id.clone(),
                            is_selected,
                            is_equipped,
                            front,
                            back,
                            "Loading...",
                            move |_, cx| {
                                if let Some(uuid) = cape_uuid {
                                    let _ = this_entity.update(cx, |this, cx| {
                                        this.selected_cape_id = Some(uuid);
                                        this.update_skin_renderer(cx);
                                        cx.notify();
                                    });
                                }
                            },
                        ).into_any_element(),
                    );
                }
                let _ = thumbnail_cache;

                let total_skins = skin_cards.len();
                let (skins_visible, has_more_skins) = if self.skins_expanded || total_skins <= INITIAL_SKINS_VISIBLE {
                    (skin_cards, false)
                } else {
                    let visible: Vec<_> = skin_cards.into_iter().take(INITIAL_SKINS_VISIBLE).collect();
                    (visible, total_skins > INITIAL_SKINS_VISIBLE)
                };

                let right_panel = v_flex()
                    .flex_1()
                    .h_full()
                    .overflow_y_scrollbar()
                    .pr_4()
                    .child(
                        h_flex()
                            .items_center()
                            .mb_4()
                            .gap_2()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::BOLD)
                                    .child(format!("{}'s Skins", profile.name)),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .mb_2()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::BOLD)
                                    .child("Owned Capes"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .flex_wrap()
                            .children(cape_cards)
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .mb_2()
                            .mt_6()
                            .child("Owned Skins"),
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .flex_wrap()
                            .children(skins_visible)
                    )
                    .when(has_more_skins, |this| {
                        this.child(
                            Button::new("show_more_skins")
                                .mt_4()
                                .label("Show more")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.skins_expanded = true;
                                    cx.notify();
                                })),
                        )
                    });

                let skin_cards_flex = h_flex()
                    .w_full()
                    .h_full()
                    .gap_6()
                    .items_start()
                    .on_mouse_up(gpui::MouseButton::Left, {
                        let skin_renderer = self.skin_renderer.clone();
                        cx.listener(move |_, _: &MouseUpEvent, _, cx| {
                            skin_renderer.update(cx, |r, _| {
                                r.is_dragging = false;
                                r.is_mouse_down = false;
                                r.last_mouse = None;
                            });
                        })
                    })
                    .on_mouse_up(gpui::MouseButton::Right, {
                        let skin_renderer = self.skin_renderer.clone();
                        cx.listener(move |_, _: &MouseUpEvent, _, cx| {
                            skin_renderer.update(cx, |r, _| {
                                r.is_dragging = false;
                                r.is_mouse_down = false;
                                r.last_mouse = None;
                            });
                        })
                    })
                    .child(left_panel)
                    .child(right_panel);

                vec![skin_cards_flex.into_any_element()]
            },
        });

        let inner = v_flex()
            .gap_4()
            .h_full()
            .child(content.flex_1().min_h_0());
        let page_type = PageType::Skins;
        let page_path = InterfaceConfig::get(cx).page_path.clone();
        let scrollable = self.scrollable(cx);
        let controls = self.controls(window, cx);
        page_layout(page_type, page_path, controls, scrollable, inner)
    }
}
