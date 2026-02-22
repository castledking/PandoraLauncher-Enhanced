use std::{path::Path, sync::Arc};

use bridge::{
    handle::BackendHandle,
    message::{MessageToBackend, MinecraftProfileInfo},
    modal_action::ModalAction,
};
use gpui::{InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, prelude::*, *};
use gpui_component::{
    Disableable, Icon, IconName, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    scroll::ScrollableElement,
    v_flex,
};

use crate::{
    component::skin_renderer::SkinRenderer,
    entity::{account::AccountEntries, minecraft_profile::MinecraftProfileEntries, DataEntities},
    ui,
    modals::upload_skin_modal,
};

enum SkinPageState {
    Loading,
    NotAuthenticated,
    Error(SharedString),
    Ready(MinecraftProfileInfo),
}

pub struct SkinsPage {
    backend_handle: BackendHandle,
    minecraft_profile: Entity<MinecraftProfileEntries>,
    accounts: Entity<AccountEntries>,
    state: SkinPageState,
    selected_skin_id: Option<Arc<str>>,
    _subscription: Subscription,
    _get_profile_task: Task<()>,
    skin_renderer: Entity<SkinRenderer>,
    _download_active_skin_task: Option<Task<()>>,
    last_rendered_skin_url: Option<String>,
    _download_active_cape_task: Option<Task<()>>,
    last_rendered_cape_url: Option<String>,
    
    thumbnail_cache: std::collections::HashMap<Arc<str>, (Arc<RenderImage>, Arc<RenderImage>)>,
    _thumbnail_tasks: std::collections::HashMap<Arc<str>, Task<()>>,
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

        let mut page = Self {
            backend_handle: data.backend_handle.clone(),
            minecraft_profile: data.minecraft_profile.clone(),
            accounts: data.accounts.clone(),
            state,
            selected_skin_id: None,
            _subscription,
            _get_profile_task: Task::ready(()),
            skin_renderer: cx.new(|_| SkinRenderer::new(None, false)),
            _download_active_skin_task: None,
            last_rendered_skin_url: None,
            _download_active_cape_task: None,
            last_rendered_cape_url: None,
            thumbnail_cache: std::collections::HashMap::new(),
            _thumbnail_tasks: std::collections::HashMap::new(),
        };
        page.load_profile(cx);
        page.update_skin_renderer(cx);
        page
    }

    fn refresh_from_entity(&mut self, cx: &mut Context<Self>) {
        let profile = self.minecraft_profile.read(cx).profile.clone();
        self.state = match profile {
            Some(p) => {
                self.selected_skin_id = p.skins.iter().find(|s| s.state.as_ref() == "ACTIVE").map(|s| s.id.clone());
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
                        this.selected_skin_id =
                            p.skins.iter().find(|s| s.state.as_ref() == "ACTIVE").map(|s| s.id.clone());
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

    fn upload_skin(&mut self, data: Arc<[u8]>, variant: Arc<str>) {
        self.backend_handle.send(MessageToBackend::UploadSkin {
            skin_data: data,
            skin_variant: variant,
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

            if let Some(active) = profile.skins.iter().find(|s| s.state.as_ref() == "ACTIVE") {
                let url = active.url.to_string();
                let is_slim = active.variant.as_ref() == "SLIM";
                if self.last_rendered_skin_url.as_deref() != Some(url.as_str()) {
                    self.last_rendered_skin_url = Some(url.clone());
                    let skin_renderer = self.skin_renderer.clone();
                    let client = cx.http_client();
                    self._download_active_skin_task = Some(cx.spawn(async move |_page, cx| {
                        if let Ok(mut response) = client.get(&url, ().into(), true).await {
                            use futures::AsyncReadExt;
                            let mut bytes = Vec::new();
                            if response.body_mut().read_to_end(&mut bytes).await.is_ok() {
                                let data: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());
                                let _ = skin_renderer.update(cx, |r, _| r.update_image(Some(data), is_slim));
                            }
                        }
                    }));
                } else {
                    self.skin_renderer.update(cx, |r, _| r.slim = is_slim);
                }
            } else {
                self.skin_renderer.update(cx, |r, _| r.update_image(None, false));
            }

            if let Some(active_cape) = profile.capes.first() {
                let url = active_cape.url.to_string();
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
                                let _ = skin_renderer.update(cx, |r, _| r.update_cape(Some(data)));
                            }
                        }
                    }));
                }
            } else {
                self.skin_renderer.update(cx, |r, _| r.update_cape(None));
                self.last_rendered_cape_url = None;
            }

            // Lazy load thumbnails
            for skin in &profile.skins {
                let id = skin.id.clone();
                let url = skin.url.to_string();
                let is_slim = skin.variant.as_ref() == "SLIM";

                if !self.thumbnail_cache.contains_key(&id) && !self._thumbnail_tasks.contains_key(&id) {
                    let client = cx.http_client();
                    let id_clone = id.clone();
                    let task = cx.spawn(async move |this, cx| {
                        if let Ok(mut response) = client.get(&url, ().into(), true).await {
                            use futures::AsyncReadExt;
                            let mut bytes = Vec::new();
                            if response.body_mut().read_to_end(&mut bytes).await.is_ok() {
                                let data: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());
                                let renderer = SkinRenderer::new(Some(data), is_slim);
                                // Render both front and back - adjust for forward-facing logic
                                // Yaw ~0.3 radians for front, PI + 0.3 for back.
                                let front = renderer.render_to_buffer_with_params(200, 200, 0.3, 0.05, true);
                                let back = renderer.render_to_buffer_with_params(200, 200, std::f32::consts::PI + 0.3, 0.05, true);
                                
                                if let (Some(f), Some(b)) = (front, back) {
                                    let _ = this.update(cx, |this, cx| {
                                        this.thumbnail_cache.insert(id_clone, (f, b));
                                        cx.notify();
                                    });
                                }
                            }
                        }
                    });
                    self._thumbnail_tasks.insert(id, task);
                }
            }
        }
    }
}

impl Render for SkinsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            SkinPageState::Error(msg) => {
                vec![
                    div().child("Error").into_any_element(),
                    div().child(msg.clone()).into_any_element(),
                ]
            },
            SkinPageState::Ready(profile) => {
                let left_panel = v_flex()
                    .w(px(350.0))
                    .h_full()
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
                        self.skin_renderer.clone()
                    );

                let add_skin_card = div()
                    .flex()
                    .flex_col()
                    .w(px(155.0))
                    .h(px(220.0))
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
                
                for skin in &profile.skins {
                    let is_active = skin.state.as_ref() == "ACTIVE";
                    let (front, back) = self.thumbnail_cache.get(&skin.id).cloned().map(|(f, b)| (Some(f), Some(b))).unwrap_or((None, None));
                    let url = skin.url.clone();
                    let variant = skin.variant.clone();
                    let this_entity = cx.entity().clone();

                    skin_cards.push(
                        crate::component::skin_card::render_skin_card(
                            skin.id.clone(),
                            is_active,
                            skin.url.clone(),
                            skin.variant.clone(),
                            front,
                            back,
                            move |_, cx| {
                                let _ = this_entity.update(cx, |this, _| {
                                    this.set_skin(url.clone(), variant.clone());
                                });
                            }
                        ).into_any_element()
                    );
                }

                let right_panel = v_flex()
                    .flex_1()
                    .h_full()
                    .overflow_y_scrollbar()
                    .pr_4()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("{}'s Skins", profile.name))
                            .mb_4(),
                    )
                    .child(div().text_lg().font_weight(FontWeight::BOLD).child("Owned Skins").mb_2())
                    .child(
                        h_flex()
                            .gap_4()
                            .flex_wrap()
                            .children(skin_cards)
                    );

                vec![h_flex().w_full().h_full().gap_6().child(left_panel).child(right_panel).into_any_element()]
            },
        });

        ui::page(cx, h_flex().gap_8().child("Skins")).child(content.h_full())
    }
}
