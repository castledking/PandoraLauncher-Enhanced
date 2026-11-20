use std::sync::Arc;

use bridge::{
    instance::{ContentFolder, InstanceID},
    meta::MetadataRequest,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    notification::NotificationType,
    skeleton::Skeleton,
    tab::{Tab, TabBar},
    text::TextView,
    v_flex,
};
use schema::{
    content::ContentSource,
    modrinth::{
        ModrinthDisclosureKind, ModrinthDisclosuresRequest, ModrinthDisclosuresResult, ModrinthProjectRequest,
        ModrinthProjectResult, ModrinthProjectType, ModrinthSideRequirement,
    },
};
use strum::IntoEnumIterator;

use crate::{
    component::error_alert::ErrorAlert,
    entity::{
        DataEntities,
        instance::ContentStates,
        metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult, FrontendMetadataState},
    },
    format_downloads,
    icon::PandoraIcon,
    pages::modrinth_page::{InstalledContent, PrimaryAction, env_display, get_primary_action, icon_for},
};

pub struct ModrinthProjectPage {
    data: DataEntities,
    project_id: SharedString,
    install_for: Option<InstanceID>,
    project: Option<Arc<ModrinthProjectResult>>,
    disclosures: Option<Arc<ModrinthDisclosuresResult>>,
    error: Option<SharedString>,
    active_tab: usize,
    can_install_latest: bool,
    specific_installed_content: enum_map::EnumMap<ContentFolder, Vec<InstalledContent>>,
    all_installed_content: Vec<InstalledContent>,
    content_states: Option<ContentStates>,
    _project_retry_task: Task<()>,
    _project_subscription: Option<Subscription>,
    _disclosures_subscription: Option<Subscription>,
}

impl ModrinthProjectPage {
    pub fn new(
        project_id: SharedString,
        install_for: Option<InstanceID>,
        data: &DataEntities,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut can_install_latest = false;
        let mut specific_installed_content: enum_map::EnumMap<ContentFolder, Vec<InstalledContent>> =
            Default::default();
        let mut all_installed_content = Vec::new();
        let mut content_states = None;

        if let Some(install_for) = install_for {
            if let Some(entry) = data.instances.read(cx).entries.get(&install_for) {
                let instance = entry.read(cx);
                let instance_content = instance.content.clone();
                content_states = Some(instance.content_states.clone());
                let loader = instance.configuration.loader;
                let minecraft_version = instance.configuration.minecraft_version;
                can_install_latest = true;

                for content_folder in ContentFolder::iter() {
                    if let Some(content) = instance_content[content_folder].read(cx) {
                        for summary in content.iter() {
                            let ContentSource::ModrinthProject {
                                project_id: other_project_id,
                            } = &summary.content_source
                            else {
                                continue;
                            };

                            if project_id.as_str() != &**other_project_id {
                                continue;
                            }

                            let installed_content = InstalledContent {
                                content_id: summary.id,
                                status: summary.update.status_if_matches(loader, minecraft_version.as_str()),
                            };
                            all_installed_content.push(installed_content);
                            specific_installed_content[content_folder].push(installed_content);
                        }
                    }

                    let content = instance_content[content_folder].clone();
                    let project_id = project_id.clone();
                    cx.observe(&content, move |page, entity, cx| {
                        let specific = &mut page.specific_installed_content[content_folder];

                        specific.clear();

                        if let Some(content) = entity.read(cx) {
                            for summary in content.iter() {
                                let ContentSource::ModrinthProject {
                                    project_id: other_project_id,
                                } = &summary.content_source
                                else {
                                    continue;
                                };

                                if project_id.as_str() != &**other_project_id {
                                    continue;
                                }

                                specific.push(InstalledContent {
                                    content_id: summary.id,
                                    status: summary.update.status_if_matches(loader, minecraft_version.as_str()),
                                });
                            }
                        }

                        page.all_installed_content.clear();
                        for content_folder in ContentFolder::iter() {
                            page.all_installed_content
                                .extend_from_slice(page.specific_installed_content[content_folder].as_slice());
                        }
                    })
                    .detach();
                }
            }
        }

        let mut page = Self {
            data: data.clone(),
            project_id,
            install_for,
            project: None,
            disclosures: None,
            error: None,
            active_tab: 0,
            can_install_latest,
            specific_installed_content,
            all_installed_content,
            content_states,
            _project_retry_task: Task::ready(()),
            _project_subscription: None,
            _disclosures_subscription: None,
        };
        page.fetch_project(cx);
        page
    }

    fn get_primary_action(&self, cx: &App) -> PrimaryAction {
        get_primary_action(self.can_install_latest, Some(&self.all_installed_content), cx)
    }

    fn fetch_project(&mut self, cx: &mut Context<Self>) {
        let request = MetadataRequest::ModrinthProject(ModrinthProjectRequest {
            project_id: Arc::from(self.project_id.as_ref()),
        });

        let state = FrontendMetadata::request(&self.data.metadata, request, cx);

        self._project_subscription = Some(cx.observe(&state, |page, state, cx| {
            page.update_project_from_metadata(state, cx);
            cx.notify();
        }));
        self.update_project_from_metadata(state, cx);

        let disclosures = FrontendMetadata::request(
            &self.data.metadata,
            MetadataRequest::ModrinthDisclosures(ModrinthDisclosuresRequest {
                project_id: Arc::from(self.project_id.as_ref()),
            }),
            cx,
        );
        self._disclosures_subscription = Some(cx.observe(&disclosures, |page, state, cx| {
            page.update_disclosures(state, cx);
            cx.notify();
        }));
        self.update_disclosures(disclosures, cx);
    }

    /// Disclosures load separately from the project, and a project page is still worth showing
    /// without them, so failing to load them just leaves the section out.
    fn update_disclosures(&mut self, state: Entity<FrontendMetadataState>, cx: &mut Context<Self>) {
        let result: FrontendMetadataResult<ModrinthDisclosuresResult> = state.read(cx).result();
        if let FrontendMetadataResult::Loaded(disclosures) = result {
            self.disclosures = Some(Arc::new(disclosures.clone()));
        }
    }

    fn update_project_from_metadata(&mut self, state: Entity<FrontendMetadataState>, cx: &mut Context<Self>) {
        self.project = None;
        self.error = None;

        let result: FrontendMetadataResult<ModrinthProjectResult> = state.read(cx).result();
        match result {
            FrontendMetadataResult::Loading => {},
            FrontendMetadataResult::Loaded(project) => {
                self.project = Some(Arc::new(project.clone()));
            },
            FrontendMetadataResult::Error(error, alive) => {
                self.error = Some(error);

                if let Some(alive) = alive {
                    self._project_retry_task = cx.spawn(async move |page, cx| {
                        alive.await_notification().await;
                        let _ = page.update(cx, |page, cx| {
                            page.fetch_project(cx);
                            cx.notify();
                        });
                    });
                }
            },
        }
    }
}

use crate::pages::page::Page;

impl Page for ModrinthProjectPage {
    fn controls(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }

    fn scrollable(&self, _cx: &App) -> bool {
        true
    }
}

impl Render for ModrinthProjectPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(content_states) = &self.content_states {
            content_states.observe_all();
        }

        let content: AnyElement = if let Some(error) = &self.error {
            v_flex()
                .p_4()
                .child(ErrorAlert::new(t::instance::content::error_loading().into(), error.clone()))
                .into_any_element()
        } else if let Some(project) = &self.project {
            let project = Arc::clone(project);

            let icon = gpui::img(SharedUri::from(
                project.icon_url.as_ref().map(|url| url.to_string()).unwrap_or_else(|| "".to_string()),
            ))
            .with_fallback(|| Skeleton::new().rounded_lg().size_20().into_any_element());

            let (env_icon, env_name) = env_display(
                project.environment.as_deref(),
                project.client_side.unwrap_or(ModrinthSideRequirement::Unknown),
                project.server_side.unwrap_or(ModrinthSideRequirement::Unknown),
            );

            let categories_el: Option<AnyElement> = {
                let cats: Vec<_> = project
                    .categories
                    .iter()
                    .flat_map(|c| c.iter())
                    .chain(project.additional_categories.iter().flat_map(|c| c.iter()))
                    .collect();

                if cats.is_empty() {
                    None
                } else {
                    let text = cats
                        .iter()
                        .map(|c| t::modrinth::category::get(c.as_str(), false).unwrap_or(c.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    Some(h_flex().gap_1().text_sm().child(PandoraIcon::Tags).child(text).into_any_element())
                }
            };

            let stats = h_flex()
                .gap_4()
                .child(h_flex().gap_1().child(PandoraIcon::Download).child(format_downloads(project.downloads)))
                .when_some(categories_el, |this, el| this.child(el));

            let info_bar = h_flex().gap_4().items_center().text_sm().child(stats);

            let mut link_row = h_flex().gap_1().flex_wrap();

            let project_id_str = project.id.clone();
            let project_type = project.project_type;

            let install_button: AnyElement = {
                let data = self.data.clone();
                let install_for = self.install_for;
                let project_name = project
                    .title
                    .clone()
                    .map(SharedString::new)
                    .unwrap_or(t::instance::content::unnamed().into());

                let primary_action = if install_for.is_some() {
                    self.get_primary_action(cx)
                } else {
                    PrimaryAction::Install
                };

                Button::new("install_project")
                    .label(primary_action.text())
                    .icon(primary_action.icon())
                    .with_variant(primary_action.button_variant())
                    .my_auto()
                    .px_6()
                    .on_click({
                        let project_name = project_name.clone();
                        let project_id_str = project_id_str.clone();
                        move |_, window, cx| {
                            if project_type != ModrinthProjectType::Other {
                                primary_action.perform(
                                    project_name.clone(),
                                    &project_id_str,
                                    project_type,
                                    install_for,
                                    None,
                                    &data,
                                    window,
                                    cx,
                                );
                            } else {
                                window.push_notification(
                                    (NotificationType::Error, t::instance::content::install::unknown_type()),
                                    cx,
                                );
                            }
                        }
                    })
                    .into_any_element()
            };
            link_row = link_row.child(install_button);

            let slug = project.slug.as_deref().unwrap_or(project.id.as_ref()).to_string();
            let project_type_str = project.project_type.as_str().to_string();
            link_row = link_row.child(
                Button::new("modrinth_web")
                    .label(t::modrinth::name())
                    .icon(PandoraIcon::ExternalLink)
                    .info()
                    .on_click({
                        let url = format!("https://modrinth.com/{}/{}", project_type_str, slug);
                        move |_, _, cx| {
                            cx.open_url(&url);
                        }
                    }),
            );

            if let Some(url) = &project.source_url {
                let url = url.clone();
                link_row = link_row.child(
                    Button::new("source")
                        .label(t::instance::content::links::source())
                        .icon(PandoraIcon::CodeXml)
                        .info()
                        .on_click(move |_, _, cx| {
                            cx.open_url(&url);
                        }),
                );
            }
            if let Some(url) = &project.issues_url {
                let url = url.clone();
                link_row = link_row.child(
                    Button::new("issues")
                        .label(t::instance::content::links::issues())
                        .icon(PandoraIcon::Bug)
                        .info()
                        .on_click(move |_, _, cx| {
                            cx.open_url(&url);
                        }),
                );
            }
            if let Some(url) = &project.wiki_url {
                let url = url.clone();
                link_row =
                    link_row.child(Button::new("wiki").label(t::instance::content::links::wiki()).info().on_click(
                        move |_, _, cx| {
                            cx.open_url(&url);
                        },
                    ));
            }
            if let Some(url) = &project.discord_url {
                let url = url.clone();
                link_row = link_row.child(
                    Button::new("discord").label(t::instance::content::links::discord()).info().on_click(
                        move |_, _, cx| {
                            cx.open_url(&url);
                        },
                    ),
                );
            }

            let license_el: Option<AnyElement> = project.license.as_ref().map(|lic| {
                let display_id = match lic.id.as_ref() {
                    "LicenseRef-All-Rights-Reserved" => "ARR".to_string(),
                    id if id.contains("LicenseRef") => id.replace("LicenseRef-", "").replace("-", " "),
                    id => id.to_string(),
                };
                let url = lic.url.as_ref().map(|u| u.to_string());

                let mut container =
                    h_flex().id("license").gap_1().text_sm().child(PandoraIcon::Scroll).child(display_id);

                if let Some(url) = url {
                    container = container.cursor_pointer().on_click(move |_, _, cx| {
                        cx.open_url(&url);
                    });
                }

                container.into_any_element()
            });

            let versions_el: Option<AnyElement> =
                project.game_versions.as_deref().filter(|v| !v.is_empty()).map(|gv| {
                    let text = if gv.len() <= 5 {
                        gv.iter().map(|v| v.as_ref()).collect::<Vec<_>>().join(", ")
                    } else {
                        t::modrinth::versions::range(
                            gv.first().map(|v| v.as_ref()).unwrap_or(""),
                            gv.last().map(|v| v.as_ref()).unwrap_or(""),
                            gv.len(),
                        )
                    };
                    h_flex().gap_1().text_sm().child(PandoraIcon::Layers).child(text).into_any_element()
                });

            let info_el: AnyElement = h_flex()
                .gap_4()
                .when_some(versions_el, |this, el| this.child(el))
                .when_some(project.loaders.as_deref(), |this, loaders| {
                    this.children(loaders.iter().map(|loader| {
                        h_flex()
                            .gap_1()
                            .when_some(icon_for(loader.id()), |this, icon| this.child(Icon::empty().path(icon)))
                            .child(loader.pretty_name())
                    }))
                })
                .child(h_flex().gap_1().child(env_icon).child(env_name))
                .when_some(license_el, |this, el| this.child(el))
                .into_any_element();

            let gallery = project.gallery.as_deref().filter(|images| !images.is_empty());
            let active_tab = if self.active_tab == 1 && gallery.is_some() {
                1
            } else {
                0
            };
            let tabs_el: AnyElement = TabBar::new("content_tabs")
                .underline()
                .selected_index(active_tab)
                .on_click(cx.listener(|this, selected_index: &usize, _window, cx| {
                    this.active_tab = *selected_index;
                    cx.notify();
                }))
                .child(Tab::new().label(t::instance::content::tabs::description()))
                .when(gallery.is_some(), |this| {
                    this.child(Tab::new().label(t::instance::content::tabs::gallery()))
                })
                .into_any_element();

            let body_el: AnyElement = if active_tab == 1 {
                v_flex()
                    .mt_2()
                    .pt_2()
                    .child(h_flex().flex_wrap().gap_3().children(gallery.into_iter().flatten().enumerate().map(
                        |(idx, img)| {
                            v_flex()
                                .rounded_lg()
                                .h_80()
                                .child(
                                    gpui::img(SharedUri::from(&img.url))
                                        .w_full()
                                        .h_72()
                                        .cursor_pointer()
                                        .rounded_t_lg()
                                        .id(("gallery_img", idx))
                                        .on_click({
                                            let url = img.url.clone();
                                            move |_, _, cx| {
                                                cx.open_url(&url);
                                            }
                                        }),
                                )
                                .child(v_flex().p_1().max_w_full().min_w_0().child(
                                    div().text_sm().child(SharedString::new(img.title.as_deref().unwrap_or_default())),
                                ))
                        },
                    )))
                    .into_any_element()
            } else if let Some(body) = &project.body
                && !body.is_empty()
            {
                v_flex()
                    .child(TextView::markdown("project_description", body.to_string()).gap_4())
                    .into_any_element()
            } else {
                v_flex()
                    .mt_2()
                    .pt_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t::instance::content::no_description()),
                    )
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
                            .line_height(relative(1.1))
                            .gap_2()
                            .child(
                                div().text_xl().overflow_hidden().child(
                                    project
                                        .title
                                        .clone()
                                        .map(SharedString::new)
                                        .unwrap_or(t::instance::content::unnamed().into()),
                                ),
                            )
                            .child(
                                div()
                                    .max_w(rems(40.0))
                                    .min_w_0()
                                    .child(project.description.clone().map(SharedString::new).unwrap_or_default()),
                            )
                            .child(info_bar),
                    ),
                )
                .child(link_row)
                .child(info_el)
                .children(self.disclosures.as_deref().and_then(|disclosures| render_disclosures(disclosures, cx)))
                .child(tabs_el)
                .child(body_el)
                .into_any_element()
        } else {
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
                                .child(
                                    h_flex()
                                        .gap_4()
                                        .mr_4()
                                        .justify_between()
                                        .child(
                                            v_flex()
                                                .w_full()
                                                .gap_2()
                                                .mr_auto()
                                                .child(Skeleton::new().h_6())
                                                .child(Skeleton::new().h_12()),
                                        )
                                        .child(Skeleton::new().h_12().w_32().rounded_md()),
                                )
                                .child(Skeleton::new().h_6().w_full().rounded_md()),
                        ),
                )
                .child(Skeleton::new().h_6().w_64().rounded_md())
                .child(Skeleton::new().h_6().w_64().rounded_lg())
                .child(Skeleton::new().h_6().w_64().rounded_md())
                .into_any_element()
        };

        content
    }
}

/// The project's content disclosures, the way Modrinth's Details panel lists them: one line each
/// saying what the project contains, with whatever detail its creator gave underneath.
fn render_disclosures(disclosures: &ModrinthDisclosuresResult, cx: &App) -> Option<AnyElement> {
    let rows: Vec<AnyElement> = disclosures.active().map(|disclosure| render_disclosure(disclosure, cx)).collect();
    if rows.is_empty() {
        return None;
    }

    Some(
        v_flex()
            .gap_2()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().warning.opacity(0.5))
            .bg(cx.theme().warning.opacity(0.06))
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .font_semibold()
                    .child(Icon::new(PandoraIcon::TriangleAlert).text_color(cx.theme().warning))
                    .child(t::instance::content::disclosures()),
            )
            .children(rows)
            .into_any_element(),
    )
}

fn render_disclosure(disclosure: &ModrinthDisclosureKind, cx: &App) -> AnyElement {
    use t::instance::content::disclosures as label;

    let joined = |items: &[Arc<str>]| -> Option<SharedString> {
        let items: Vec<&str> = items.iter().map(|item| item.trim()).filter(|item| !item.is_empty()).collect();
        (!items.is_empty()).then(|| items.join(", ").into())
    };
    let note = |note: &Option<Arc<str>>| -> Option<SharedString> {
        note.as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .map(|note| SharedString::from(note.to_string()))
    };

    let (title, details): (SharedString, Vec<SharedString>) = match disclosure {
        ModrinthDisclosureKind::Advertisements { note: text } => {
            (label::advertisements().into(), note(text).into_iter().collect())
        },
        ModrinthDisclosureKind::PaidFeatures { features } => {
            (label::paid_features().into(), joined(features).into_iter().collect())
        },
        ModrinthDisclosureKind::Telemetry {
            consent,
            data_collected,
        } => {
            let consent = match consent.as_deref() {
                Some("opt_in") => Some(label::telemetry::opt_in()),
                Some("opt_out") => Some(label::telemetry::opt_out()),
                Some("always_active") => Some(label::telemetry::always_active()),
                _ => None,
            };
            let title = match consent {
                Some(consent) => format!("{} ({consent})", label::telemetry()).into(),
                None => label::telemetry().into(),
            };
            (title, joined(data_collected).into_iter().collect())
        },
        ModrinthDisclosureKind::AiContent { note: text, uses } => {
            let uses: Vec<&str> = uses
                .iter()
                .map(|usage| match &**usage {
                    "code" => label::ai_content::code(),
                    "assets" => label::ai_content::assets(),
                    "text" => label::ai_content::text(),
                    other => other,
                })
                .collect();
            let title = if uses.is_empty() {
                label::ai_content().into()
            } else {
                format!("{} ({})", label::ai_content(), uses.join(", ")).into()
            };
            (title, note(text).into_iter().collect())
        },
        ModrinthDisclosureKind::AiFunctionality { note: text } => {
            (label::ai_functionality().into(), note(text).into_iter().collect())
        },
        ModrinthDisclosureKind::EpilepsyTriggers { note: text } => {
            (label::epilepsy_triggers().into(), note(text).into_iter().collect())
        },
        ModrinthDisclosureKind::SystemInteractions {
            note: text,
            interactions,
        } => (
            label::system_interactions().into(),
            joined(interactions).into_iter().chain(note(text)).collect(),
        ),
        ModrinthDisclosureKind::DerivativeWork { sources } => {
            let details = sources
                .iter()
                .map(|source| {
                    let mut line = source.label.to_string();
                    if let Some(link) = source.link.as_deref().filter(|link| !link.is_empty()) {
                        line.push_str(&format!(" ({link})"));
                    }
                    if let Some(note) = source.note.as_deref().filter(|note| !note.is_empty()) {
                        line.push_str(&format!(": {note}"));
                    }
                    SharedString::from(line)
                })
                .collect();
            (label::derivative_work().into(), details)
        },
        ModrinthDisclosureKind::Archived { note: text } => (label::archived().into(), note(text).into_iter().collect()),
        ModrinthDisclosureKind::Unknown => (SharedString::default(), Vec::new()),
    };

    v_flex()
        .text_sm()
        .child(div().font_medium().child(title))
        .children(
            details
                .into_iter()
                .map(|detail| div().text_color(cx.theme().muted_foreground).child(detail)),
        )
        .into_any_element()
}
