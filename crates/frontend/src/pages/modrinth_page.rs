
use std::{ops::Range, sync::Arc};

use bridge::handle::BackendHandle;
use gpui::{prelude::*, *};
use gpui_component::{button::{Button, ButtonGroup}, h_flex, input::{Input, InputEvent, InputState}, list::{ListDelegate, ListItem, ListState}, scroll::{Scrollable, Scrollbar, ScrollbarState}, skeleton::Skeleton, v_flex, v_virtual_list, ActiveTheme, Icon, IconName, Selectable, Sizable, StyledExt, VirtualListScrollHandle};
use rust_i18n::t;
use schema::modrinth::{ModrinthError, ModrinthHit, ModrinthRequest, ModrinthResult, ModrinthSearchRequest, ModrinthSideRequirement};

use crate::{entity::{modrinth::{FrontendModrinthData, FrontendModrinthDataState}, DataEntities}, ts, ui};

#[derive(PartialEq)]
enum ProjectType {
    Mod,
    Modpack,
    Resourcepack,
    Shader,
    Datapack,
}

pub struct ModrinthSearchPage {
    backend_handle: BackendHandle,
    modrinth: Entity<FrontendModrinthData>,
    hits: Vec<ModrinthHit>,
    loading: Option<Subscription>,
    total_hits: usize,
    search_state: Entity<InputState>,
    _search_input_subscription: Subscription,
    project_type: ProjectType,
    last_search: Arc<str>,
    scroll_state: ScrollbarState,
    scroll_handle: UniformListScrollHandle,
}

impl ModrinthSearchPage {
    pub fn new(data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_state = cx.new(|cx| InputState::new(window, cx).placeholder("Search mods...").clean_on_escape());
        
        let _search_input_subscription =
            cx.subscribe_in(&search_state, window, Self::on_search_input_event);
        
        let mut page = Self {
            backend_handle: data.backend_handle.clone(),
            modrinth: data.modrinth.clone(),
            hits: Vec::new(),
            loading: None,
            total_hits: 1,
            search_state,
            _search_input_subscription,
            project_type: ProjectType::Mod,
            last_search: Arc::from(""),
            scroll_state: ScrollbarState::default(),
            scroll_handle: UniformListScrollHandle::new(),
        };
        page.load_more(cx);
        page
    }
    
    fn on_search_input_event(
        &mut self,
        state: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) { 
        let InputEvent::Change = event else {
            return;
        };
        
        let search = state.read(cx).text().to_string();
        let search = search.trim();
        
        if &*self.last_search == search {
            return;
        }
        
        let search: Arc<str> = Arc::from(search);
        self.last_search = search.clone();
        self.reload(cx);
    }
    
    fn set_project_type(&mut self, project_type: ProjectType, cx: &mut Context<Self>) {
        if self.project_type == project_type {
            return;
        }
        self.project_type = project_type;
        self.reload(cx);
    }
    
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = None;
        self.hits.clear();
        self.total_hits = 1;
        self.load_more(cx);
    }
    
    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading.is_some() {
            return;
        }
        
        let query = if self.last_search.is_empty() {
            None
        } else {
            Some(self.last_search.clone())
        };
        
        let project_type = match self.project_type {
            ProjectType::Mod => "mod",
            ProjectType::Modpack => "modpack",
            ProjectType::Resourcepack => "resourcepack",
            ProjectType::Shader => "shader",
            ProjectType::Datapack => "datapack",
        };
        
        let request = ModrinthSearchRequest {
            query,
            facets: Some(format!("[[\"client_side!=unsupported\"],[\"project_type={}\"]]", project_type).into()),
            index: schema::modrinth::ModrinthSearchIndex::Relevance,
            offset: self.hits.len(),
            limit: 20,
        };
        
        let data = FrontendModrinthData::request(&self.modrinth, ModrinthRequest::Search(request), cx);
        
        if let FrontendModrinthDataState::Loaded { result, .. } = data.read(cx) {
            self.apply_search_data(result);
            return;
        }
        
        let subscription = cx.observe(&data, |page, data, cx| {
            if let FrontendModrinthDataState::Loaded { result, .. } = data.read(cx) {
                page.apply_search_data(result);
                page.loading = None;
            }
        });
        self.loading = Some(subscription);
    }
    
    fn apply_search_data(&mut self, result: &Result<ModrinthResult, ModrinthError>) {
        match result {
            Ok(value) => {
                let ModrinthResult::Search(search_result) = value else {
                    unreachable!();
                };
                
                self.hits.extend(search_result.hits.iter().cloned());
                self.total_hits = search_result.total_hits;
            },
            Err(error) => todo!(),
        }
    }
}

impl Render for ModrinthSearchPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_load_more = self.total_hits > self.hits.len();
        let scroll_handle = self.scroll_handle.clone();
        let scroll_state = self.scroll_state.clone();
        
        let item_count = self.hits.len() + if can_load_more { 1 } else { 0 };
        
        let list = h_flex()
            .size_full()
            .overflow_y_hidden()
            .child(uniform_list(
                "uniform-list",
                item_count,
                cx.processor(move |this, visible_range: Range<usize>, window, cx| {
                    let theme = cx.theme();
                    let mut should_load_more = false;
                    let items = visible_range.map(|index| {
                        let Some(hit) = this.hits.get(index) else {
                            should_load_more = true;
                            return div().pl_3().pt_3().child(Skeleton::new().w_full().h(px(28.0*4.0)).rounded_lg());
                        };
                        
                        let image = if let Some(icon_url) = &hit.icon_url && !icon_url.is_empty() {
                            gpui::img(SharedUri::from(icon_url)).with_fallback(|| {
                                Skeleton::new().rounded_lg().size_16().into_any_element()
                            })
                        } else {
                            gpui::img(ImageSource::Resource(Resource::Embedded("images/default_world.png".into())))
                        };
                        
                        let name = hit.title.as_ref().map(Arc::clone)
                            .map(SharedString::new).unwrap_or(SharedString::new_static("Unnamed"));
                        let description = hit.description.as_ref().map(Arc::clone)
                            .map(SharedString::new).unwrap_or(SharedString::new_static("No Description"));
                        
                        let client_side = hit.client_side.unwrap_or(ModrinthSideRequirement::Unknown);
                        let server_side = hit.server_side.unwrap_or(ModrinthSideRequirement::Unknown);
                        
                        let (env_icon, env_name) = match (client_side, server_side) {
                            (ModrinthSideRequirement::Required, ModrinthSideRequirement::Required) => {
                                (Icon::empty().path("icons/globe.svg"), ts!("client_and_server"))
                            },
                            (ModrinthSideRequirement::Required, ModrinthSideRequirement::Unsupported) => {
                                (Icon::empty().path("icons/computer.svg"), ts!("client_only"))
                            },
                            (ModrinthSideRequirement::Required, ModrinthSideRequirement::Optional) => {
                                (Icon::empty().path("icons/computer.svg"), ts!("client_only_server_optional"))
                            },
                            (ModrinthSideRequirement::Unsupported, ModrinthSideRequirement::Required) => {
                                (Icon::empty().path("icons/router.svg"), ts!("server_only"))
                            },
                            (ModrinthSideRequirement::Optional, ModrinthSideRequirement::Required) => {
                                (Icon::empty().path("icons/router.svg"), ts!("server_only_client_optional"))
                            },
                            (ModrinthSideRequirement::Optional, ModrinthSideRequirement::Optional) => {
                                (Icon::empty().path("icons/globe.svg"), ts!("client_or_server"))
                            },
                            _ => {
                                (Icon::empty().path("icons/cpu.svg"), ts!("unknown_environment"))
                            }
                        };
                        
                        let environment = h_flex().gap_1().font_bold().child(env_icon).child(env_name);
                        
                        let categories = hit.display_categories.iter().flat_map(|categories| {
                            categories.iter().map(|category| {
                                let icon = icon_for(&*category).unwrap_or("icons/diamond.svg");
                                let icon = Icon::empty().path(icon);
                                let translated_category = ts!(category.as_str());
                                h_flex().gap_0p5().child(icon).child(translated_category)
                            })
                        });
                        
                        let download_icon = Icon::empty().path("icons/download.svg");
                        let downloads = h_flex().gap_0p5().child(download_icon).child(format_downloads(hit.downloads));
                        
                        let item = h_flex()
                            .rounded_lg()
                            .px_4()
                            .py_2()
                            .gap_4()
                            .h_32()
                            .bg(theme.background)
                            .border_color(theme.border)
                            .border_1()
                            .size_full()
                            .child(image.rounded_lg().size_16().min_w_16().min_h_16())
                            .child(v_flex().h(px(104.0)).flex_grow().gap_1().overflow_hidden()
                                .child(div().line_clamp(1).text_lg().child(name))
                                .child(div().flex_auto().line_height(px(20.0)).line_clamp(2).child(description))
                                .child(h_flex().gap_2p5().children(std::iter::once(environment).chain(categories))))
                            .child(div().child(downloads));
                        
                        div().pl_3().pt_3().child(item)
                    }).collect();
                    
                    if should_load_more {
                        this.load_more(cx);
                    }
                    
                    items
                })).size_full().track_scroll(scroll_handle.clone())
            )
            .child(div().w_3().h_full().py_3().child(Scrollbar::uniform_scroll(&scroll_state, &scroll_handle)));
        
        let theme = cx.theme();
        let content = v_flex()
            .size_full()
            .gap_3()
            .child(Input::new(&self.search_state))
            .child(div().size_full().rounded_lg().border_1().border_color(theme.border).child(list));
        
        
        let type_button_group = ButtonGroup::new("type").vertical(true).outline()
            .child(Button::new("mods").label("Mods").selected(self.project_type == ProjectType::Mod))
            .child(Button::new("modpacks").label("Modpacks").selected(self.project_type == ProjectType::Modpack))
            .child(Button::new("resourcepacks").label("Resourcepacks").selected(self.project_type == ProjectType::Resourcepack))
            .child(Button::new("shaders").label("Shaders").selected(self.project_type == ProjectType::Shader))
            .child(Button::new("datapacks").label("Datapacks").selected(self.project_type == ProjectType::Datapack))
            .on_click(cx.listener(|page, clicked: &Vec<usize>, _, cx| {
                match clicked[0] {
                    0 => page.set_project_type(ProjectType::Mod, cx),
                    1 => page.set_project_type(ProjectType::Modpack, cx),
                    2 => page.set_project_type(ProjectType::Resourcepack, cx),
                    3 => page.set_project_type(ProjectType::Shader, cx),
                    4 => page.set_project_type(ProjectType::Datapack, cx),
                    _ => {}
                }
            }));
        
        let parameters = v_flex()
            .h_full()
            .gap_3()
            .child(type_button_group)
            .child("Test2");
        
        ui::page(cx, "Modrinth").child(h_flex().size_full().p_3().gap_3().child(parameters).child(content))
    }
}

fn format_downloads(downloads: usize) -> String {
    if downloads >= 1_000_000_000 {
        format!("{}B Downloads", (downloads / 10_000_000) as f64 / 100.0)
    } else if downloads >= 1_000_000 {
        format!("{}M Downloads", (downloads / 10_000) as f64 / 100.0)
    } else if downloads >= 10_000 {
        format!("{}K Downloads", (downloads / 10) as f64 / 100.0)
    } else {
        format!("{} Downloads", downloads)
    }
}

fn icon_for(str: &str) -> Option<&'static str> {
    match str {
        "forge" => Some("icons/anvil.svg"),
        "fabric" => Some("icons/scroll.svg"),
        "neoforge" => Some("icons/cat.svg"),
        "quilt" => Some("icons/grid-2x2.svg"),
        "adventure" => Some("icons/compass.svg"),
        "cursed" => Some("icons/bug.svg"),
        "decoration" => Some("icons/house.svg"),
        "economy" => Some("icons/dollar-sign.svg"),
        "equipment" | "combat" => Some("icons/swords.svg"),
        "food" => Some("icons/carrot.svg"),
        "game-mechanics" => Some("icons/sliders-vertical.svg"),
        "library" | "items" => Some("icons/book.svg"),
        "magic" => Some("icons/wand.svg"),
        "management" => Some("icons/server.svg"),
        "minigame" => Some("icons/award.svg"),
        "mobs" | "entities" => Some("icons/cat.svg"),
        "optimization" => Some("icons/zap.svg"),
        "social" => Some("icons/message-circle.svg"),
        "storage" => Some("icons/archive.svg"),
        "technology" => Some("icons/hard-drive.svg"),
        "transportation" => Some("icons/truck.svg"),
        "utility" => Some("icons/briefcase.svg"),
        "world-generation" | "locale" => Some("icons/globe.svg"),
        "audio" => Some("icons/headphones.svg"),
        "blocks" | "rift" => Some("icons/box.svg"),
        "core-shaders" => Some("icons/cpu.svg"),
        "fonts" => Some("icons/type.svg"),
        "gui" => Some("icons/panels-top-left.svg"),
        "models" => Some("icons/layers.svg"),
        "cartoon" => Some("icons/brush.svg"),
        "fantasy" => Some("icons/wand-sparkles.svg"),
        "realistic" => Some("icons/camera.svg"),
        "semi-realistic" => Some("icons/film.svg"),
        "vanilla-like" => Some("icons/ice-cream-cone.svg"),
        "atmosphere" => Some("icons/cloud-sun-rain.svg"),
        "colored-lighting" => Some("icons/palette.svg"),
        "foliage" => Some("icons/tree-pine.svg"),
        "path-tracing" => Some("icons/waypoints.svg"),
        "pbr" => Some("icons/lightbulb.svg"),
        "reflections" => Some("icons/flip-horizontal-2.svg"),
        "shadows" => Some("icons/mountain.svg"),
        "challenging" => Some("icons/chart-no-axes-combined.svg"),
        "kitchen-sink" => Some("icons/bath.svg"),
        "lightweight" | "liteloader" => Some("icons/feather.svg"),
        "multiplayer" => Some("icons/users.svg"),
        "quests" => Some("icons/network.svg"),
        _ => None
    }
}
