use gpui::*;
use gpui_component::{
    Sizable,
    button::{Button, ButtonVariants},
};

use crate::{
    component::{generic_title_bar::TitleBar, page_path::PagePath},
    entity::DataEntities,
    icon::PandoraIcon,
    modals::update_prompt::PendingUpdate,
};

#[derive(IntoElement)]
pub struct MainTitleBar {
    pub page_path: PagePath,
    pub controls: AnyElement,
    pub update: Option<PendingUpdate>,
    pub data: DataEntities,
}

impl RenderOnce for MainTitleBar {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let update_button = self.update.map(|update| {
            let button = Button::new("update").success().compact().small().ml_2();

            match update {
                PendingUpdate::Available(update) => {
                    button.label(t::system::update::available()).icon(PandoraIcon::Download).on_click({
                        let send = self.data.backend_handle.clone();
                        move |_, window, cx| {
                            crate::modals::update_prompt::open_update_prompt(update.clone(), send.clone(), window, cx);
                        }
                    })
                },
                PendingUpdate::Restart(restart) => {
                    button.label(t::system::update::restart::button()).icon(PandoraIcon::Download).on_click({
                        let data = self.data.clone();
                        move |_, window, cx| {
                            crate::modals::update_prompt::open_restart_prompt(
                                restart.clone(),
                                data.clone(),
                                window,
                                cx,
                            );
                        }
                    })
                },
            }
            .into_any_element()
        });

        TitleBar {
            left_content: vec![
                div().overflow_hidden().pr_8().child(self.page_path).into_any_element(),
                self.controls,
            ],
            right_content: update_button.into_iter().collect(),
            content_only: !crate::root::should_render_custom_titlebar(),
        }
    }
}
