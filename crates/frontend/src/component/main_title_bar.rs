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
            let label = match update {
                PendingUpdate::Available(_) => t::system::update::available(),
                PendingUpdate::Restart(_) => t::system::update::restart::button(),
            };
            Button::new("update")
                .success()
                .compact()
                .small()
                .ml_2()
                .label(label)
                .icon(PandoraIcon::Download)
                .on_click({
                    let data = self.data.clone();
                    move |_, window, cx| {
                        crate::modals::update_prompt::open_update_dialog(update.clone(), &data, window, cx);
                    }
                })
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
