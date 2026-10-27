use std::{collections::BTreeMap, rc::Rc, sync::Arc};

use bridge::{
    content_label::{global_content_keys, modpack_file_source},
    instance::{ContentFolder, ContentSummary, ContentType, InstanceID},
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, DropdownMenu},
    v_flex,
};
use schema::{
    content::ContentSource,
    content_label::{ContentLabel, ContentLabelEdit, GlobalContentLabel, edit_global_label},
};

use crate::{
    component::content_label::{self, LabelMenu, LabelTarget},
    entity::{
        DataEntities,
        instance::{InstanceAddedEvent, InstanceRemovedEvent},
    },
    icon::PandoraIcon,
    interface_config::InterfaceConfig,
    settings::{SettingGroup, SettingItem, SettingItemWidget, SettingPage},
};

pub(super) fn create_page(data: &DataEntities, window: &mut Window, cx: &mut App) -> SettingPage {
    let editor = cx.new(|cx| GlobalLabels::new(data.clone(), window, cx));
    SettingPage {
        title: t::settings::content_labels,
        groups: vec![SettingGroup {
            title: None,
            items: vec![
                SettingItem {
                    title: t::instance::content::categories,
                    description: t::instance::content::label::manage::description,
                    widget: SettingItemWidget::Any(Rc::new(|_, _| {
                        Button::new("manage-content-categories")
                            .label(t::instance::content::label::manage_categories())
                            .icon(PandoraIcon::Tags)
                            .on_click(|_, window, cx| crate::modals::content_label::open_manage_categories(window, cx))
                            .into_any_element()
                    })),
                    ..Default::default()
                },
                SettingItem {
                    title: t::settings::content_labels::global,
                    description: t::settings::content_labels::description,
                    widget: SettingItemWidget::FullWidth(Rc::new(move |_, _| editor.clone().into_any_element())),
                    ..Default::default()
                },
            ]
            .into(),
            searched_items: None,
        }]
        .into(),
        searched_groups: None,
    }
}

struct GlobalLabels {
    data: DataEntities,
    search: Entity<InputState>,
    observed: BTreeMap<InstanceID, Vec<Subscription>>,
}

impl GlobalLabels {
    fn new(data: DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t::settings::content_labels::search()));
        cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&data.instances, |_, _, _: &InstanceAddedEvent, cx| cx.notify()).detach();
        cx.subscribe(&data.instances, |_, _, _: &InstanceRemovedEvent, cx| cx.notify()).detach();
        Self {
            data,
            search,
            observed: BTreeMap::new(),
        }
    }

    fn candidates(&mut self, cx: &mut Context<Self>) -> Vec<GlobalContentLabel> {
        // Begin with saved assignments so they remain editable even after their last installation
        // is removed. Installed versions are then folded into one row per content identity.
        let mut rows = InterfaceConfig::get(cx).global_content_labels.clone();
        let mut identities: BTreeMap<Arc<str>, usize> = rows
            .iter()
            .enumerate()
            .flat_map(|(index, row)| row.keys.iter().cloned().map(move |key| (key, index)))
            .collect();
        let mut add =
            |folder: ContentFolder, source: &ContentSource, summary: Option<&ContentSummary>, filename: &str| {
                let keys = global_content_keys(folder, source, summary, filename);
                let index = keys.iter().find_map(|key| identities.get(key).copied()).unwrap_or_else(|| {
                    rows.push(GlobalContentLabel {
                        keys: Vec::new(),
                        name: summary.and_then(|summary| summary.name.clone()).unwrap_or_else(|| {
                            filename
                                .strip_suffix(".disabled")
                                .unwrap_or(filename)
                                .rsplit('/')
                                .next()
                                .unwrap_or(filename)
                                .into()
                        }),
                        label: ContentLabel::default(),
                    });
                    rows.len() - 1
                });
                for key in keys {
                    identities.insert(key.clone(), index);
                    if !rows[index].keys.contains(&key) {
                        rows[index].keys.push(key);
                    }
                }
            };

        let instances = self.data.instances.read(cx).entries.clone();
        self.observed.retain(|id, _| instances.contains_key(id));
        for (id, entity) in instances {
            let instance = entity.read(cx).clone();
            self.observed.entry(id).or_insert_with(|| {
                let mut subscriptions = vec![cx.observe(&entity, |_, _, cx| cx.notify())];
                for (_, content) in &instance.content {
                    subscriptions.push(cx.observe(content, |_, _, cx| cx.notify()));
                }
                subscriptions
            });
            instance.content_states.observe_all();
            for (folder, content) in &instance.content {
                let Some(content) = content.read(cx) else {
                    continue;
                };
                for item in content.iter() {
                    let summary = &item.content_summary;
                    let files = match &summary.extra {
                        ContentType::ModrinthModpack { files, .. } => files,
                        ContentType::CurseforgeModpack {
                            files, unknown_files, ..
                        } => {
                            for file in unknown_files.iter() {
                                add(
                                    ContentFolder::Mods,
                                    &ContentSource::CurseforgeProject {
                                        project_id: file.project_id,
                                    },
                                    None,
                                    &t::instance::content::file_id(file.file_id),
                                );
                            }
                            files
                        },
                        _ => {
                            add(folder, &item.content_source, Some(summary), &item.filename);
                            continue;
                        },
                    };
                    for file in files.iter() {
                        let path = file.path();
                        let folder = match path.as_ref().map(|path| path.as_str().split('/').next().unwrap_or_default())
                        {
                            Some("mods") | None => ContentFolder::Mods,
                            Some("resourcepacks") => ContentFolder::ResourcePacks,
                            Some("shaderpacks") => ContentFolder::Shaders,
                            Some("plugins") => ContentFolder::Plugins,
                            _ => continue,
                        };
                        add(folder, &modpack_file_source(summary, file), file.summary.as_deref(), file.path.as_str());
                    }
                }
            }
        }
        rows.sort_by(|a, b| lexical_sort::natural_lexical_cmp(&a.name, &b.name).then_with(|| a.keys.cmp(&b.keys)));
        rows
    }

    fn render_row(&self, index: usize, row: &GlobalContentLabel, cx: &App) -> impl IntoElement {
        let config = InterfaceConfig::get(cx);
        let color = content_label::resolve_color(&row.label, &config.content_categories);
        let menu = Rc::new(LabelMenu {
            target: LabelTarget::Global {
                keys: row.keys.clone().into(),
                name: row.name.clone(),
            },
            labels: vec![row.label.clone()],
            preview_name: SharedString::from(row.name.clone()),
            backend_handle: self.data.backend_handle.clone(),
        });
        let keys = row.keys.clone();
        let name = row.name.clone();
        let identity = row.keys.first().map(|key| key.splitn(3, ':').collect::<Vec<_>>()).unwrap_or_default();
        let folder = match identity.first().copied() {
            Some("mods") => t::instance::content::mods(),
            Some("plugins") => t::server::plugins(),
            Some("resourcepacks") => t::instance::content::resourcepacks(),
            _ => t::instance::content::shaders(),
        };
        let kind = match identity.get(1).copied() {
            Some("modrinth") => "Modrinth",
            Some("curseforge") => "CurseForge",
            Some("id") => "ID",
            Some("name") => t::settings::content_labels::name(),
            _ => t::settings::content_labels::filename(),
        };
        let subtitle = format!("{folder} · {kind}: {}", identity.get(2).copied().unwrap_or_default());
        let mut row_element = h_flex()
            .id(("global-label", index))
            .h(px(62.))
            .w_full()
            .gap_2()
            .px_2()
            .rounded(cx.theme().radius)
            .child(content_label::swatch(color, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().child(SharedString::from(row.name.clone())))
                    .child(div().truncate().text_xs().text_color(cx.theme().muted_foreground).child(subtitle)),
            )
            .when_some(row.label.category.as_deref(), |this, category| {
                this.child(content_label::category_chip(
                    category,
                    content_label::category_color(Some(category), &config.content_categories),
                    cx,
                ))
            })
            .child(
                Button::new(("edit-global-label", index))
                    .small()
                    .outline()
                    .label(t::settings::content_labels::edit())
                    .icon(PandoraIcon::ChevronDown)
                    .dropdown_menu({
                        let menu = menu.clone();
                        move |popup, window, cx| menu.build(popup, window, cx)
                    }),
            )
            .when(!row.label.is_empty(), |this| {
                this.child(
                    Button::new(("remove-global-label", index))
                        .small()
                        .danger()
                        .icon(PandoraIcon::Trash2)
                        .tooltip(t::settings::content_labels::remove())
                        .on_click(move |_, _, cx| {
                            edit_global_label(
                                &mut InterfaceConfig::get_mut(cx).global_content_labels,
                                &keys,
                                name.clone(),
                                &ContentLabelEdit::Clear,
                            );
                            cx.refresh_windows();
                        }),
                )
            })
            .context_menu(move |popup, window, cx| menu.build(popup, window, cx));
        if let Some(color) = color {
            let (background, _) = content_label::row_tint(color, false, cx);
            row_element = row_element.bg(background);
        }
        row_element
    }
}

impl Render for GlobalLabels {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let rows: Rc<[_]> =
            self.candidates(cx)
                .into_iter()
                .filter(|row| {
                    query.is_empty()
                        || row.name.to_lowercase().contains(&query)
                        || row.keys.iter().any(|key| key.to_lowercase().contains(&query))
                        || row.label.category.as_deref().is_some_and(|category| {
                            content_label::category_name(category).to_lowercase().contains(&query)
                        })
                })
                .collect();
        let count = rows.len();
        v_flex()
            .w_full()
            .gap_3()
            .child(Input::new(&self.search).prefix(gpui_component::Icon::new(PandoraIcon::Search)))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t::settings::content_labels::fallback_hint()),
            )
            .when(count == 0, |this| {
                this.child(
                    div()
                        .py_4()
                        .text_color(cx.theme().muted_foreground)
                        .child(t::settings::content_labels::empty()),
                )
            })
            .when(count > 0, |this| {
                this.child(
                    uniform_list(
                        "global-content-labels",
                        count,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range.map(|index| this.render_row(index, &rows[index], cx).into_any_element()).collect()
                        }),
                    )
                    .w_full()
                    .h(px((count.min(7) as f32) * 62.)),
                )
            })
    }
}
