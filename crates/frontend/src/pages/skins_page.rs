use std::{path::Path, sync::Arc};

use bridge::{
    handle::BackendHandle,
    message::{MessageToBackend, MinecraftProfileInfo},
    modal_action::ModalAction,
};
use gpui::{prelude::*, *};
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
    custom_skin_url: Entity<InputState>,
    custom_skin_variant: Arc<str>,
    _subscription: Subscription,
    _get_profile_task: Task<()>,
    custom_skin_file_data: Option<Arc<[u8]>>,
    custom_skin_file_name: Option<SharedString>,
    upload_error: Option<SharedString>,
    _select_file_task: Task<()>,
    skin_renderer: Entity<SkinRenderer>,
    _download_active_skin_task: Option<Task<()>>,
    last_rendered_skin_url: Option<String>,
    _download_active_cape_task: Option<Task<()>>,
    last_rendered_cape_url: Option<String>,
}

impl SkinsPage {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let profile = data.minecraft_profile.read(cx).profile.clone();

        let state = match profile {
            Some(p) => SkinPageState::Ready(p),
            None => SkinPageState::Loading,
        };

        let custom_skin_url =
            cx.new(|cx| InputState::new(window, cx).placeholder("Custom skin URL (e.g. https://.../skin.png)"));

        let _subscription = cx.subscribe(&data.minecraft_profile, |this, _, _, cx| {
            this.refresh_from_entity(cx);
            cx.notify();
        });

        let mut page = Self {
            backend_handle: data.backend_handle.clone(),
            minecraft_profile: data.minecraft_profile.clone(),
            accounts: data.accounts.clone(),
            state,
            selected_skin_id: None,
            custom_skin_url,
            custom_skin_variant: "CLASSIC".into(),
            _subscription,
            _get_profile_task: Task::ready(()),
            custom_skin_file_data: None,
            custom_skin_file_name: None,
            upload_error: None,
            _select_file_task: Task::ready(()),
            skin_renderer: cx.new(|_| SkinRenderer::new(None, false)),
            _download_active_skin_task: None,
            last_rendered_skin_url: None,
            _download_active_cape_task: None,
            last_rendered_cape_url: None,
        };
        page.load_profile(cx);
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

    fn select_skin_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::new_static("Select Skin PNG (64x64 or 64x32)")),
        });

        let this_entity = cx.entity();
        self._select_file_task = window.spawn(cx, async move |cx| {
            let Ok(result) = receiver.await else { return };
            let Ok(Some(paths)) = result else { return };
            let Some(path) = paths.first() else { return };
            let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();

            let Ok(bytes) = std::fs::read(path) else { return };
            let Ok(img) = image::load_from_memory(&bytes) else {
                return;
            };

            let (w, h) = (img.width(), img.height());
            if (w == 64 && h == 64) || (w == 64 && h == 32) {
                let rgba = img.to_rgba8();
                let data: Arc<[u8]> = Arc::from(rgba.into_raw());
                let _ = cx.update_window_entity(&this_entity, move |this, _window, cx| {
                    this.custom_skin_file_data = Some(data);
                    this.custom_skin_file_name = Some(file_name.into());
                    this.upload_error = None;
                    this.update_skin_renderer(cx);
                });
            }
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
        if let Some(custom) = &self.custom_skin_file_data {
            let is_slim = self.custom_skin_variant.as_ref() == "SLIM";
            self.skin_renderer.update(cx, |r, _| r.update_image(Some(custom.clone()), is_slim));
            return;
        }

        if let SkinPageState::Ready(profile) = &self.state {
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
        }
    }
}

impl Render for SkinsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_skin_renderer(cx);
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
                    .w(px(300.0))
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

                let skins_list = h_flex().gap_4().flex_wrap().children(profile.skins.iter().map(|skin| {
                    let is_active = skin.state.as_ref() == "ACTIVE";
                    let url = skin.url.clone();
                    let variant = skin.variant.clone();

                    v_flex()
                        .gap_2()
                        .child(gpui::img(SharedUri::from(url.to_string())).w_24().h_24().rounded_md().bg(rgb(0x202020)))
                        .child(
                            Button::new(SharedString::from(format!("set_skin_{}", skin.id)))
                                .label(if is_active { "Active" } else { "Set Active" })
                                .when(is_active, |b| b.success())
                                .disabled(is_active)
                                .on_click(cx.listener(move |this, _, _, _| {
                                    this.set_skin(url.clone(), variant.clone());
                                })),
                        )
                }));

                let custom_skin_section = v_flex()
                    .gap_4()
                    .mt_8()
                    .child(div().text_lg().font_weight(FontWeight::BOLD).child("Upload Custom Skin"))
                    .child(h_flex().gap_4().child(Input::new(&self.custom_skin_url).flex_1()).child(
                        Button::new("set-custom-url").label("Set from URL").success().on_click(cx.listener(
                            |this, _, _, cx| {
                                let url = this.custom_skin_url.read(cx).value();
                                if !url.is_empty() {
                                    this.set_skin(url.into(), this.custom_skin_variant.clone());
                                }
                            },
                        )),
                    ))
                    .child(
                        h_flex()
                            .gap_4()
                            .items_center()
                            .child(
                                Button::new("select-file").label("Select Local File...").icon(IconName::File).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.select_skin_file(window, cx);
                                    }),
                                ),
                            )
                            .child(
                                div()
                                    .child(if let Some(err) = &self.upload_error {
                                        err.clone()
                                    } else if let Some(name) = &self.custom_skin_file_name {
                                        SharedString::from(format!("Selected: {}", name))
                                    } else {
                                        SharedString::from("No file selected")
                                    })
                                    .text_color(if self.upload_error.is_some() {
                                        gpui::red()
                                    } else {
                                        gpui::rgba(0xaaaaaaff).into()
                                    }),
                            )
                            .child(
                                Button::new("upload-file")
                                    .label("Upload File")
                                    .success()
                                    .disabled(self.custom_skin_file_data.is_none() || self.upload_error.is_some())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(data) = &this.custom_skin_file_data {
                                            this.upload_skin(data.clone(), this.custom_skin_variant.clone());
                                        }
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .mt_2()
                            .child(
                                Button::new("variant-classic")
                                    .label("Classic Model")
                                    .when(self.custom_skin_variant.as_ref() == "CLASSIC", |b| b.info())
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.custom_skin_variant = "CLASSIC".into();
                                    })),
                            )
                            .child(
                                Button::new("variant-slim")
                                    .label("Slim Model")
                                    .when(self.custom_skin_variant.as_ref() == "SLIM", |b| b.info())
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.custom_skin_variant = "SLIM".into();
                                    })),
                            ),
                    );

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
                    .child(skins_list)
                    .child(custom_skin_section);

                vec![h_flex().w_full().h_full().gap_6().child(left_panel).child(right_panel).into_any_element()]
            },
        });

        ui::page(cx, h_flex().gap_8().child("Skins")).child(content.h_full())
    }
}
