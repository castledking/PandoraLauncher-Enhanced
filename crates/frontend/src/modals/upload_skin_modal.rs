use std::sync::Arc;

use bridge::{
    handle::BackendHandle,
    message::MessageToBackend,
    modal_action::ModalAction,
};
use gpui::{InteractiveElement, ParentElement, SharedString, Styled, Window, prelude::*, *};
use gpui_component::{
    ActiveTheme,
    button::{Button, ButtonVariants},
    dialog::Dialog,
    h_flex,
    input::{Input, InputState},
    v_flex,
    Disableable, IconName, WindowExt,
};
use image::ImageEncoder;

use crate::{component::skin_renderer::SkinRenderer, icon::PandoraIcon};

fn detect_skin_variant(bytes: &[u8]) -> &'static str {
    if let Ok(img) = image::load_from_memory(bytes) {
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        if w != 64 {
            return "CLASSIC";
        }
        
        let mut has_pixels = false;
        for y in 20..32 {
            for x in 54..56 {
                if x < w as usize && y < h as usize {
                    let pixel = rgba.get_pixel(x as u32, y as u32);
                    if pixel[3] != 0 {
                        has_pixels = true;
                        break;
                    }
                }
            }
            if has_pixels {
                break;
            }
        }
        
        if has_pixels { "CLASSIC" } else { "SLIM" }
    } else {
        "CLASSIC"
    }
}

pub struct UploadSkinModal {
    backend_handle: BackendHandle,
    custom_skin_url: Entity<InputState>,
    variant_mode: Arc<str>,
    detected_variant: Arc<str>,
    selected_file_data: Option<Arc<[u8]>>,
    selected_file_name: Option<SharedString>,
    upload_error: Option<SharedString>,
    preview_front: Option<Arc<RenderImage>>,
    preview_back: Option<Arc<RenderImage>>,
    _select_file_task: Task<()>,
    _preview_task: Task<()>,
    url_section_expanded: bool,
}

impl UploadSkinModal {
    pub fn new(
        backend_handle: BackendHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let custom_skin_url =
            cx.new(|cx| InputState::new(window, cx).placeholder("Custom skin URL (e.g. https://.../skin.png)"));

        Self {
            backend_handle,
            custom_skin_url,
            variant_mode: "AUTO".into(),
            detected_variant: "CLASSIC".into(),
            selected_file_data: None,
            selected_file_name: None,
            upload_error: None,
            preview_front: None,
            preview_back: None,
            _select_file_task: Task::ready(()),
            _preview_task: Task::ready(()),
            url_section_expanded: false,
        }
    }

    fn effective_variant(&self) -> Arc<str> {
        match self.variant_mode.as_ref() {
            "AUTO" => self.detected_variant.clone(),
            other => other.into(),
        }
    }

    fn submit_variant(&self) -> Arc<str> {
        if self.selected_file_data.is_none() && self.variant_mode.as_ref() == "AUTO" {
            "AUTO".into()
        } else {
            self.effective_variant()
        }
    }

    fn generate_preview(&mut self, skin_data: Arc<[u8]>, cx: &mut Context<Self>) {
        let is_slim = self.effective_variant().as_ref() == "SLIM";
        let this_entity = cx.entity();
        self._preview_task = cx.spawn(async move |_, cx| {
            let renderer = SkinRenderer::new(Some(skin_data), is_slim);
            let front = renderer.render_to_buffer_with_params(200, 200, 0.3, 0.05, true);
            let back = renderer.render_to_buffer_with_params(200, 200, std::f32::consts::PI + 0.3, 0.05, true);

            if let (Some(f), Some(b)) = (front, back) {
                let _ = this_entity.update(cx, |this, cx| {
                    this.preview_front = Some(f);
                    this.preview_back = Some(b);
                    cx.notify();
                });
            }
        });
    }

    fn select_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                let _ = cx.update_window_entity(&this_entity, move |this, _, cx| {
                    this.upload_error = Some("Invalid image file".into());
                    cx.notify();
                });
                return;
            };

            let (w, h) = (img.width(), img.height());
            if (w == 64 && h == 64) || (w == 64 && h == 32) {
                let mut png_data = Vec::new();
                let encoder = image::codecs::png::PngEncoder::new(&mut png_data);
                if let Err(err) = encoder.write_image(&img.to_rgba8(), w, h, image::ExtendedColorType::Rgba8) {
                    let _ = cx.update_window_entity(&this_entity, move |this, _, cx| {
                        this.upload_error = Some(format!("Failed to process image: {err}").into());
                        cx.notify();
                    });
                    return;
                }
                let data: Arc<[u8]> = Arc::from(png_data);
                
                let detected_variant = detect_skin_variant(&bytes);
                
                let _ = cx.update_window_entity(&this_entity, move |this, _window, cx| {
                    this.selected_file_data = Some(data.clone());
                    this.selected_file_name = Some(file_name.into());
                    this.detected_variant = detected_variant.into();
                    if this.variant_mode.as_ref() == "AUTO" {
                        this.variant_mode = this.detected_variant.clone();
                    }
                    this.upload_error = None;
                    this.preview_front = None;
                    this.preview_back = None;
                    this.generate_preview(data, cx);
                    cx.notify();
                });
            } else {
                let _ = cx.update_window_entity(&this_entity, move |this, _, cx| {
                    this.upload_error = Some("Skins must be 64x64 or 64x32".into());
                    cx.notify();
                });
            }
        });
    }

    pub fn render(&mut self, modal: Dialog, _window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        let has_preview = self.preview_front.is_some();

        let variant_buttons = h_flex()
            .gap_2()
            .when(!has_preview, |this| {
                this.child(
                    Button::new("variant-auto")
                        .label("Auto")
                        .when(self.variant_mode.as_ref() == "AUTO", |b| b.info())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.variant_mode = "AUTO".into();
                            if let Some(data) = this.selected_file_data.clone() {
                                this.generate_preview(data, cx);
                            }
                        })),
                )
            })
            .child(
                Button::new("variant-classic")
                    .label("Classic")
                    .when(self.variant_mode.as_ref() == "CLASSIC", |b| b.info())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.variant_mode = "CLASSIC".into();
                        if let Some(data) = this.selected_file_data.clone() {
                            this.generate_preview(data, cx);
                        }
                    })),
            )
            .child(
                Button::new("variant-slim")
                    .label("Slim")
                    .when(self.variant_mode.as_ref() == "SLIM", |b| b.info())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.variant_mode = "SLIM".into();
                        if let Some(data) = this.selected_file_data.clone() {
                            this.generate_preview(data, cx);
                        }
                    })),
            );

        let url_expanded = self.url_section_expanded;
        let url_section = v_flex()
            .gap_2()
            .child(
                Button::new("toggle-url-section")
                    .icon(if url_expanded { PandoraIcon::ChevronDown } else { PandoraIcon::ChevronRight })
                    .when(!url_expanded, |b| b.outline())
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_sm().child("Add from URL"))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .px_1()
                                    .py_0p5()
                                    .rounded(cx.theme().radius)
                                    .bg(hsla(0.14, 0.7, 0.45, 0.25))
                                    .text_color(hsla(0.14, 0.8, 0.55, 1.0))
                                    .child("Experimental"),
                            ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.url_section_expanded = !this.url_section_expanded;
                        cx.notify();
                    })),
            )
            .when(url_expanded, |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .pl_6()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Input::new(&self.custom_skin_url).flex_1())
                                .child(
                                    Button::new("set-url")
                                        .label("Upload from URL")
                                        .success()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let url = this.custom_skin_url.read(cx).value();
                                            if !url.is_empty() {
                                                this.backend_handle.send(MessageToBackend::AddOwnedSkinFromUrl {
                                                    skin_url: url.into(),
                                                    skin_variant: this.submit_variant(),
                                                    modal_action: ModalAction::default(),
                                                });
                                            }
                                        })),
                                ),
                        ),
                )
            });

        let preview_widget = if let Some(front) = &self.preview_front {
            let front_img = front.clone();
            let back_img = self.preview_back.clone();
            Some(
                div()
                    .id("skin-preview")
                    .w(px(120.0))
                    .h(px(120.0))
                    .rounded_lg()
                    .bg(gpui::rgba(0x2d2d35ff))
                    .overflow_hidden()
                    .flex_shrink_0()
                    .group("preview-card")
                    .child(
                        div()
                            .size_full()
                            .group_hover("preview-card", |style| style.invisible())
                            .child(
                                canvas(
                                    move |_, _, _| (),
                                    {
                                        let img = front_img.clone();
                                        move |bounds, _, window, _| {
                                            let _ = window.paint_image(bounds, gpui::Corners::default(), img.clone(), 0, false);
                                        }
                                    },
                                )
                                .size_full(),
                            ),
                    )
                    .when_some(back_img, |this, back| {
                        this.child(
                            div()
                                .size_full()
                                .absolute()
                                .inset_0()
                                .invisible()
                                .group_hover("preview-card", |style| style.visible())
                                .child(
                                    canvas(
                                        move |_, _, _| (),
                                        move |bounds, _, window, _| {
                                            let _ = window.paint_image(bounds, gpui::Corners::default(), back.clone(), 0, false);
                                        },
                                    )
                                    .size_full(),
                                ),
                        )
                    })
                    .relative()
            )
        } else {
            None
        };

        let file_section = v_flex()
            .gap_2()
            .child(div().text_sm().child("Add from File"))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("select-file")
                            .label("Select Local File...")
                            .icon(IconName::File)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.select_file(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .child(if let Some(err) = &self.upload_error {
                                err.clone()
                            } else if let Some(name) = &self.selected_file_name {
                                name.clone()
                            } else {
                                "No file selected".into()
                            })
                            .text_sm()
                            .text_color(if self.upload_error.is_some() { hsla(0.0, 1.0, 0.5, 1.0) } else { hsla(0.0, 0.0, 0.7, 1.0) })
                    )
                    .child(
                        Button::new("upload-file")
                            .label("Upload")
                            .success()
                            .disabled(self.selected_file_data.is_none() || self.upload_error.is_some())
                            .on_click(cx.listener(|this, _, _, _| {
                                if let Some(data) = &this.selected_file_data {
                                    this.backend_handle.send(MessageToBackend::AddOwnedSkin {
                                        skin_data: data.clone(),
                                        skin_variant: this.submit_variant(),
                                        modal_action: ModalAction::default(),
                                    });
                                }
                            })),
                    ),
            );

        modal
            .title("Upload Skin")
            .child(
                v_flex()
                    .gap_6()
                    .child(
                        h_flex()
                            .gap_4()
                            .child(
                                v_flex().gap_2().flex_1()
                                    .child(div().text_sm().child("Model Type"))
                                    .child(variant_buttons)
                            )
                            .when_some(preview_widget, |this, preview| {
                                this.child(preview)
                            })
                    )
                    .child(url_section)
                    .child(file_section)
            )
    }
}

pub fn open(
    backend_handle: BackendHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let state = cx.new(|cx| UploadSkinModal::new(backend_handle, window, cx));

    window.open_dialog(cx, move |modal, window, cx| {
        let modal = modal.w(px(500.0));
        state.update(cx, |state, cx| state.render(modal, window, cx))
    });
}
