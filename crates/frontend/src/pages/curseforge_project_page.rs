//! A CurseForge project's page, the counterpart of the Modrinth one: what the project is, its links
//! and screenshots, and the same install button its search result has.

use std::sync::Arc;

use bridge::{
    instance::{ContentFolder, InstanceID},
    meta::MetadataRequest,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme,
    button::{Button, ButtonVariants},
    h_flex,
    skeleton::Skeleton,
    tab::{Tab, TabBar},
    text::TextView,
    v_flex,
};
use rustc_hash::FxHashMap;
use schema::{
    content::ContentSource,
    curseforge::{
        CurseforgeDescriptionRequest, CurseforgeDescriptionResult, CurseforgeModDetails, CurseforgeModLoaderType,
        CurseforgeModRequest, CurseforgeModResult,
    },
};
use strum::IntoEnumIterator;
use ustr::Ustr;

use crate::{
    component::error_alert::ErrorAlert,
    entity::{
        DataEntities,
        instance::ContentStates,
        metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult, FrontendMetadataState},
    },
    format_downloads,
    icon::PandoraIcon,
    pages::curseforge_page::{InstalledContent, PrimaryAction, get_primary_action},
};

type ContentEntity = Entity<Option<Arc<[bridge::instance::InstanceContentSummary]>>>;

pub struct CurseforgeProjectPage {
    data: DataEntities,
    project_id: u32,
    install_for: Option<InstanceID>,
    project: Option<Arc<CurseforgeModDetails>>,
    description: Option<Arc<str>>,
    error: Option<SharedString>,
    active_tab: usize,
    can_install_latest: bool,
    /// What's installed from this project in `install_for`, by content folder, so the install
    /// button can offer an update or a reinstall instead.
    installed: enum_map::EnumMap<ContentFolder, Vec<InstalledContent>>,
    content_states: Option<ContentStates>,
    _retry_tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl CurseforgeProjectPage {
    pub fn new(
        project_id: u32,
        install_for: Option<InstanceID>,
        data: &DataEntities,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut page = Self {
            data: data.clone(),
            project_id,
            install_for,
            project: None,
            description: None,
            error: None,
            active_tab: 0,
            can_install_latest: false,
            installed: Default::default(),
            content_states: None,
            _retry_tasks: Vec::new(),
            _subscriptions: Vec::new(),
        };
        page.watch_installed(cx);
        page.fetch(cx);
        page
    }

    fn watch_installed(&mut self, cx: &mut Context<Self>) {
        let Some(install_for) = self.install_for else {
            return;
        };
        let Some(entry) = self.data.instances.read(cx).entries.get(&install_for).cloned() else {
            return;
        };
        let instance = entry.read(cx);
        self.content_states = Some(instance.content_states.clone());
        self.can_install_latest = true;
        let loader = instance.configuration.loader;
        let minecraft_version = instance.configuration.minecraft_version;
        let contents = instance.content.clone();
        let project_id = self.project_id;

        for content_folder in ContentFolder::iter() {
            let content = contents[content_folder].clone();
            let collect = move |page: &mut Self, content: &ContentEntity, cx: &mut Context<Self>| {
                let installed = &mut page.installed[content_folder];
                installed.clear();
                if let Some(content) = content.read(cx) {
                    for summary in content.iter() {
                        if summary.content_source == (ContentSource::CurseforgeProject { project_id }) {
                            installed.push(InstalledContent {
                                content_id: summary.id,
                                status: summary.update.status_if_matches(loader, minecraft_version.as_str()),
                            });
                        }
                    }
                }
            };
            collect(self, &content, cx);
            self._subscriptions.push(cx.observe(&content, move |page, content, cx| {
                collect(page, &content, cx);
                cx.notify();
            }));
        }
    }

    fn get_primary_action(&self, cx: &App) -> PrimaryAction {
        let installed: Vec<InstalledContent> = self.installed.values().flatten().copied().collect();
        let mut by_project = FxHashMap::default();
        if !installed.is_empty() {
            by_project.insert(self.project_id, installed);
        }
        get_primary_action(self.project_id, self.can_install_latest, &by_project, cx)
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        let project = FrontendMetadata::request(
            &self.data.metadata,
            MetadataRequest::CurseforgeMod(CurseforgeModRequest {
                mod_id: self.project_id,
            }),
            cx,
        );
        let description = FrontendMetadata::request(
            &self.data.metadata,
            MetadataRequest::CurseforgeDescription(CurseforgeDescriptionRequest {
                mod_id: self.project_id,
            }),
            cx,
        );

        self._subscriptions.push(cx.observe(&project, |page, state, cx| {
            page.update_project(state, cx);
            cx.notify();
        }));
        self._subscriptions.push(cx.observe(&description, |page, state, cx| {
            page.update_description(state, cx);
            cx.notify();
        }));
        self.update_project(project, cx);
        self.update_description(description, cx);
    }

    fn update_project(&mut self, state: Entity<FrontendMetadataState>, cx: &mut Context<Self>) {
        let result: FrontendMetadataResult<CurseforgeModResult> = state.read(cx).result();
        match result {
            FrontendMetadataResult::Loading => {},
            FrontendMetadataResult::Loaded(result) => {
                self.error = None;
                self.project = Some(Arc::new(result.data.clone()));
            },
            FrontendMetadataResult::Error(error, alive) => {
                self.error = Some(error);
                if let Some(alive) = alive {
                    self._retry_tasks.push(cx.spawn(async move |page, cx| {
                        alive.await_notification().await;
                        _ = page.update(cx, |page, cx| {
                            page.fetch(cx);
                            cx.notify();
                        });
                    }));
                }
            },
        }
    }

    fn update_description(&mut self, state: Entity<FrontendMetadataState>, cx: &mut Context<Self>) {
        let result: FrontendMetadataResult<CurseforgeDescriptionResult> = state.read(cx).result();
        // The description is a nicety: the page is still useful without it, so a failure just
        // leaves the summary
        if let FrontendMetadataResult::Loaded(result) = result {
            self.description = result.data.clone();
        }
    }
}

use crate::pages::page::Page;

impl Page for CurseforgeProjectPage {
    fn controls(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }

    fn scrollable(&self, _cx: &App) -> bool {
        true
    }
}

impl Render for CurseforgeProjectPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(content_states) = &self.content_states {
            content_states.observe_all();
        }

        if let Some(error) = &self.error {
            return v_flex()
                .p_4()
                .child(ErrorAlert::new(t::instance::content::error_loading().into(), error.clone()))
                .into_any_element();
        }
        let Some(project) = self.project.clone() else {
            return loading_skeleton().into_any_element();
        };
        let hit = &project.hit;

        let icon = match hit.logo.as_ref().filter(|logo| !logo.thumbnail_url.is_empty()) {
            Some(logo) => gpui::img(SharedUri::from(logo.thumbnail_url.clone()))
                .with_fallback(|| Skeleton::new().rounded_lg().size_20().into_any_element()),
            None => gpui::img(ImageSource::Resource(Resource::Embedded("images/default_mod.png".into()))),
        };

        let authors = hit.authors.iter().map(|author| &*author.name).collect::<Vec<_>>().join(", ");
        let categories = hit
            .categories
            .iter()
            .filter(|category| !category.is_class)
            .map(|category| &*category.name)
            .collect::<Vec<_>>()
            .join(", ");

        let stats = h_flex()
            .gap_4()
            .text_sm()
            .child(h_flex().gap_1().child(PandoraIcon::Download).child(format_downloads(hit.download_count)))
            .when(!categories.is_empty(), |this| {
                this.child(h_flex().gap_1().child(PandoraIcon::Tags).child(categories))
            });

        // The same button, with the same rules, as the project's search result
        let primary_action = self.get_primary_action(cx);
        let install_button = Button::new("install_project")
            .label(primary_action.text())
            .icon(primary_action.icon())
            .with_variant(primary_action.button_variant())
            .px_6()
            .on_click({
                let data = self.data.clone();
                let hit = hit.clone();
                let install_for = self.install_for;
                move |_, window, cx| primary_action.perform(&hit, install_for, None, &data, window, cx)
            });

        let link = |id: &'static str, label: SharedString, icon: Option<PandoraIcon>, url: &Arc<str>| {
            let url = url.to_string();
            Button::new(id)
                .label(label)
                .when_some(icon, |button, icon| button.icon(icon))
                .info()
                .on_click(move |_, _, cx| cx.open_url(&url))
        };
        let links = h_flex()
            .gap_1()
            .flex_wrap()
            .child(install_button)
            .when_some(project.links.website(), |this, url| {
                this.child(link("website", t::curseforge::name().into(), Some(PandoraIcon::ExternalLink), url))
            })
            .when_some(project.links.source(), |this, url| {
                this.child(link(
                    "source",
                    t::instance::content::links::source().into(),
                    Some(PandoraIcon::CodeXml),
                    url,
                ))
            })
            .when_some(project.links.issues(), |this, url| {
                this.child(link("issues", t::instance::content::links::issues().into(), Some(PandoraIcon::Bug), url))
            })
            .when_some(project.links.wiki(), |this, url| {
                this.child(link("wiki", t::instance::content::links::wiki().into(), None, url))
            });

        let info = h_flex()
            .gap_4()
            .text_sm()
            .when_some(game_versions(&project), |this, versions| {
                this.child(h_flex().gap_1().child(PandoraIcon::Layers).child(versions))
            })
            .children(loaders(&project).into_iter().map(|loader| div().child(loader)));

        let has_gallery = !project.screenshots.is_empty();
        let active_tab = if has_gallery { self.active_tab } else { 0 };
        let tabs = TabBar::new("content_tabs")
            .underline()
            .selected_index(active_tab)
            .on_click(cx.listener(|page, selected: &usize, _, cx| {
                page.active_tab = *selected;
                cx.notify();
            }))
            .child(Tab::new().label(t::instance::content::tabs::description()))
            .when(has_gallery, |this| this.child(Tab::new().label(t::instance::content::tabs::gallery())));

        let body = if active_tab == 1 {
            h_flex()
                .flex_wrap()
                .gap_3()
                .pt_2()
                .children(project.screenshots.iter().enumerate().map(|(index, screenshot)| {
                    let url = screenshot.url.to_string();
                    v_flex()
                        .rounded_lg()
                        .h_80()
                        .child(
                            gpui::img(SharedUri::from(screenshot.url.clone()))
                                .id(("screenshot", index))
                                .w_full()
                                .h_72()
                                .rounded_t_lg()
                                .cursor_pointer()
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                        .child(
                            div()
                                .p_1()
                                .text_sm()
                                .child(SharedString::new(screenshot.title.as_deref().unwrap_or_default())),
                        )
                }))
                .into_any_element()
        } else if let Some(description) = self.description.as_ref().filter(|description| !description.is_empty()) {
            TextView::html("project_description", description.to_string()).into_any_element()
        } else if !hit.summary.is_empty() {
            div().pt_2().child(SharedString::new(hit.summary.clone())).into_any_element()
        } else {
            div()
                .pt_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t::instance::content::no_description())
                .into_any_element()
        };

        v_flex()
            .p_4()
            .gap_3()
            .w_full()
            .child(
                h_flex().gap_4().child(icon.rounded_lg().size_24().min_w_24().min_h_24()).child(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .line_height(relative(1.1))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_end()
                                .child(div().text_xl().child(SharedString::new(hit.name.clone())))
                                .when(!authors.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(t::instance::content::by(&authors)),
                                    )
                                }),
                        )
                        .child(div().max_w(rems(40.0)).child(SharedString::new(hit.summary.clone())))
                        .child(stats),
                ),
            )
            .child(links)
            .child(info)
            .child(tabs)
            .child(body)
            .into_any_element()
    }
}

/// The Minecraft versions the project has files for, newest first as CurseForge lists them.
fn game_versions(project: &CurseforgeModDetails) -> Option<SharedString> {
    let mut versions: Vec<Ustr> = Vec::new();
    for index in project.hit.latest_files_indexes.iter() {
        if !versions.contains(&index.game_version) {
            versions.push(index.game_version);
        }
    }
    match versions.len() {
        0 => None,
        1..=5 => Some(versions.iter().map(|version| version.as_str()).collect::<Vec<_>>().join(", ").into()),
        count => Some(t::modrinth::versions::range(versions[count - 1].as_str(), versions[0].as_str(), count).into()),
    }
}

fn loaders(project: &CurseforgeModDetails) -> Vec<&'static str> {
    let mut loaders = Vec::new();
    for index in project.hit.latest_files_indexes.iter() {
        let loader = CurseforgeModLoaderType::from_u32(index.mod_loader.unwrap_or(0));
        if loader != CurseforgeModLoaderType::Any && !loaders.contains(&loader.pretty_name()) {
            loaders.push(loader.pretty_name());
        }
    }
    loaders
}

fn loading_skeleton() -> impl IntoElement {
    v_flex()
        .p_4()
        .gap_3()
        .w_full()
        .child(
            h_flex()
                .gap_4()
                .child(Skeleton::new().rounded_lg().size_24().min_w_24().min_h_24())
                .child(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .child(Skeleton::new().h_6().w_64())
                        .child(Skeleton::new().h_12())
                        .child(Skeleton::new().h_6().w_32()),
                ),
        )
        .child(Skeleton::new().h_8().w_64().rounded_md())
        .child(Skeleton::new().h_6().w_64().rounded_md())
}
