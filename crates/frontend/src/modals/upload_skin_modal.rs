use std::sync::Arc;

use bridge::{
    handle::BackendHandle,
    message::MessageToBackend,
    modal_action::ModalAction,
};
use gpui::{InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariants},
    dialog::Dialog,
    h_flex,
    input::{Input, InputState},
    v_flex,
    IconName, Selectable, Disableable, WindowExt,
};
use image::ImageEncoder;

fn detect_skin_variant(bytes: &[u8]) -> &'static str {
    use image::GenericImageView;
    
    if let Ok(img) = image::load_from_memory(bytes) {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        if w != 64 {
            return "CLASSIC";
        }
        
        // Check arm region at x=54, y=20 (2x12 pixels)
        // Classic arms: area at x=54 has pixels (4px wide arm extends there)
        // Slim arms: area at x=54 is transparent (arm is at x=50, only 3px wide)
        let mut has_pixels = false;
        for y in 20..32 {
            for x in 54..56 {
                if x < w as usize && y < h as usize {
                    let pixel = rgba.get_pixel(x as u32, y as u32);
                    if pixel[3] != 0 { // alpha != 0
                        has_pixels = true;
                        break;
                    }
                }
            }
            if has_pixels {
                break;
            }
        }
        
        // If pixels found in this region → Classic, otherwise → Slim
        if has_pixels { "CLASSIC" } else { "SLIM" }
    } else {
        "CLASSIC"
    }
}

pub struct UploadSkinModal {
    backend_handle: BackendHandle,
    custom_skin_url: Entity<InputState>,
    variant: Arc<str>,
    selected_file_data: Option<Arc<[u8]>>,
    selected_file_name: Option<SharedString>,
    upload_error: Option<SharedString>,
    _select_file_task: Task<()>,
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
            variant: "CLASSIC".into(),
            selected_file_data: None,
            selected_file_name: None,
            upload_error: None,
            _select_file_task: Task::ready(()),
        }
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
                encoder.write_image(&img.to_rgba8(), w, h, image::ExtendedColorType::Rgba8).unwrap();
                let data: Arc<[u8]> = Arc::from(png_data);
                
                // Auto-detect skin variant
                let detected_variant = detect_skin_variant(&bytes);
                
                let _ = cx.update_window_entity(&this_entity, move |this, _window, cx| {
                    this.selected_file_data = Some(data);
                    this.selected_file_name = Some(file_name.into());
                    this.variant = detected_variant.into();
                    this.upload_error = None;
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
        let variant_buttons = h_flex()
            .gap_2()
            .child(
                Button::new("variant-classic")
                    .label("Classic Model")
                    .when(self.variant.as_ref() == "CLASSIC", |b| b.info())
                    .on_click(cx.listener(|this, _, _, _| {
                        this.variant = "CLASSIC".into();
                    })),
            )
            .child(
                Button::new("variant-slim")
                    .label("Slim Model")
                    .when(self.variant.as_ref() == "SLIM", |b| b.info())
                    .on_click(cx.listener(|this, _, _, _| {
                        this.variant = "SLIM".into();
                    })),
            );

        let url_section = v_flex()
            .gap_2()
            .child(div().text_sm().child("Add from URL"))
            .child(
                h_flex().gap_2().child(Input::new(&self.custom_skin_url).flex_1()).child(
                    Button::new("set-url").label("Upload from URL").success().on_click(cx.listener(|this, _, _, cx| {
                        let url = this.custom_skin_url.read(cx).value();
                        if !url.is_empty() {
                            this.backend_handle.send(MessageToBackend::SetSkin {
                                skin_url: url.into(),
                                skin_variant: this.variant.clone(),
                                modal_action: ModalAction::default(),
                            });
                        }
                    })),
                ),
            );

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
                                    this.backend_handle.send(MessageToBackend::UploadSkin {
                                        skin_data: data.clone(),
                                        skin_variant: this.variant.clone(),
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
                        v_flex().gap_2().child(div().text_sm().child("Model Type")).child(variant_buttons)
                    )
                    .child(url_section)
                    .child(file_section)
            )
            .confirm()
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
