use std::{
    hash::{DefaultHasher, Hash, Hasher},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use bridge::{
    content_label::{global_content_keys, modpack_file_source},
    handle::BackendHandle,
    instance::{
        ContentFolder, ContentSummary, ContentType, InstanceContentID, InstanceContentSummary, InstanceID,
        UNKNOWN_CONTENT_SUMMARY,
    },
    message::MessageToBackend,
    modal_action::ModalAction,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IndexPath, Sizable, Size,
    button::{Button, ButtonVariants},
    h_flex,
    list::{ListDelegate, ListItem, ListState},
    menu::ContextMenuExt,
    spinner::Spinner,
    switch::Switch,
    v_flex,
};
use parking_lot::Mutex;
use rustc_hash::FxHashSet;
use schema::{
    content::ContentSource,
    content_label::{ContentLabel, GlobalContentLabel, child_label_keys, find_child_label, find_global_label},
    loader::Loader,
    text_component::FlatTextComponent,
};
use strum::IntoEnumIterator;
use ustr::Ustr;

use crate::{
    component::{
        content_label::{self, LabelMenu, LabelTarget},
        error_alert::ErrorAlert,
    },
    entity::DataEntities,
    icon::PandoraIcon,
    interface_config::{ContentListDisplay, InstanceContentSortKey, InterfaceConfig},
    png_render_cache,
};

#[derive(Clone)]
struct ContentEntryChild {
    summary: Arc<ContentSummary>,
    parent_filename_hash: u64,
    parent: InstanceContentID,
    path: Arc<str>,
    filesize: u64,
    lowercase_search_keys: Arc<[Arc<str>]>,
    disabled_default: bool,
    enabled: bool,
    parent_enabled: bool,
    disabled_third_party_downloads: bool,
    is_missing: bool,
    label: ContentLabel,
    /// What the label is filed under in the modpack's auxiliary file, best first.
    label_keys: Arc<[Arc<str>]>,
}

impl ContentEntryChild {
    pub fn compare_by(&self, other: &ContentEntryChild, key: InstanceContentSortKey) -> std::cmp::Ordering {
        match key {
            InstanceContentSortKey::Name => {
                let name_a = self.summary.name.as_deref().or(self.summary.id.as_deref()).unwrap_or(&*self.path);
                let name_b = other.summary.name.as_deref().or(other.summary.id.as_deref()).unwrap_or(&*other.path);
                lexical_sort::natural_lexical_cmp(name_a, name_b)
            },
            InstanceContentSortKey::ModId => {
                let name_a = self.summary.id.as_deref().or(self.summary.name.as_deref()).unwrap_or(&*self.path);
                let name_b = other.summary.id.as_deref().or(other.summary.name.as_deref()).unwrap_or(&*other.path);
                lexical_sort::natural_lexical_cmp(name_a, name_b)
            },
            InstanceContentSortKey::Filename => {
                let name_a = &*self.path;
                let name_b = &*other.path;
                lexical_sort::natural_lexical_cmp(name_a, name_b)
            },
            InstanceContentSortKey::ModifiedTime => std::cmp::Ordering::Equal,
            InstanceContentSortKey::FileSize => self.filesize.cmp(&other.filesize).reverse(),
            InstanceContentSortKey::Category => {
                content_label::compare_categories(self.label.category.as_deref(), other.label.category.as_deref())
            },
        }
    }
}

enum SummaryOrChild {
    Summary(InstanceContentSummary),
    Child(ContentEntryChild),
}

pub struct ContentListDelegate {
    id: InstanceID,
    content_folder: ContentFolder,
    for_loader: Loader,
    for_version: Ustr,
    backend_handle: BackendHandle,
    data: DataEntities,
    sort_key: InstanceContentSortKey,
    enabled_first: bool,
    loading: bool,
    content: Vec<InstanceContentSummary>,
    global_labels: Vec<GlobalContentLabel>,
    searched: Option<Vec<SummaryOrChild>>,
    children: Vec<Vec<ContentEntryChild>>,
    /// Whether any row is a modpack whose files can be expanded below it.
    has_expandable_rows: bool,
    expanded: Arc<AtomicUsize>,
    confirming_delete: Arc<Mutex<FxHashSet<u64>>>,
    confirming_delete_children: Arc<Mutex<FxHashSet<u64>>>,
    updating: Arc<Mutex<FxHashSet<u64>>>,
    last_query: SharedString,
    selected: FxHashSet<u64>,
    selected_range: FxHashSet<u64>,
    last_clicked_non_range: Option<u64>,
}

impl ContentListDelegate {
    pub fn new(
        id: InstanceID,
        content_folder: ContentFolder,
        data: &DataEntities,
        for_loader: Loader,
        for_version: Ustr,
        sort_key: InstanceContentSortKey,
        enabled_first: bool,
        updating: Arc<Mutex<FxHashSet<u64>>>,
    ) -> Self {
        Self {
            id,
            content_folder,
            for_loader,
            for_version,
            backend_handle: data.backend_handle.clone(),
            data: data.clone(),
            sort_key,
            enabled_first,
            loading: true,
            content: Vec::new(),
            global_labels: Vec::new(),
            searched: None,
            children: Vec::new(),
            has_expandable_rows: false,
            expanded: Arc::new(AtomicUsize::new(0)),
            confirming_delete: Default::default(),
            confirming_delete_children: Default::default(),
            updating,
            last_query: SharedString::new_static(""),
            selected: FxHashSet::default(),
            selected_range: FxHashSet::default(),
            last_clicked_non_range: None,
        }
    }

    pub fn set_sort_options(&mut self, sort_key: InstanceContentSortKey, enabled_first: bool) {
        self.sort_key = sort_key;
        self.enabled_first = enabled_first;
    }

    /// How this list's rows are laid out. Read from the config on every render rather than kept
    /// here, so every list showing the same kind of content agrees.
    fn display(&self, cx: &App) -> ContentListDisplay {
        InterfaceConfig::get(cx).content_list_display(self.content_folder)
    }

    /// The label menu for a right-click on the row `element_id`. Like the row's own buttons, it
    /// acts on the whole selection when the row is part of it, and on the row alone otherwise.
    fn label_menu_for(&self, element_id: u64) -> Option<LabelMenu> {
        let clicked = self.content.iter().find(|summary| summary.filename_hash == element_id)?;
        let targets: Vec<&InstanceContentSummary> = if self.is_selected(element_id) {
            self.content.iter().filter(|summary| self.is_selected(summary.filename_hash)).collect()
        } else {
            vec![clicked]
        };

        Some(LabelMenu {
            target: LabelTarget::Content {
                instance: self.id,
                content_ids: targets.iter().map(|summary| summary.id).collect(),
            },
            labels: targets.iter().map(|summary| summary.label.clone()).collect(),
            preview_name: SharedString::from(display_filename(&clicked.filename).to_string()),
            backend_handle: self.backend_handle.clone(),
        })
    }

    pub fn render_summary(
        &self,
        summary: &InstanceContentSummary,
        selected: bool,
        expand_index: Option<usize>,
        cx: &mut Context<ListState<Self>>,
    ) -> ListItem {
        let display = self.display(cx);
        let compact = display.is_compact();
        let button_size = if compact { Size::Small } else { Size::Medium };

        let (tint, category_color) = {
            let categories = &InterfaceConfig::get(cx).content_categories;
            (
                content_label::resolve_color(&summary.label, categories),
                content_label::category_color(summary.label.category.as_deref(), categories),
            )
        };

        let id = self.id;
        let content_id = summary.id;
        let element_id = summary.filename_hash;

        let delete_button = if self.confirming_delete.lock().contains(&element_id) {
            Button::new(("delete", element_id)).danger().icon(PandoraIcon::Check).on_click({
                let backend_handle = self.backend_handle.clone();
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let delegate = this.delegate();
                    if delegate.is_selected(element_id) {
                        let content_ids = delegate
                            .content
                            .iter()
                            .filter_map(|summary| delegate.is_selected(summary.filename_hash).then(|| summary.id))
                            .collect();

                        backend_handle.send(MessageToBackend::DeleteContent { id, content_ids });
                    } else {
                        backend_handle.send(MessageToBackend::DeleteContent {
                            id,
                            content_ids: vec![content_id],
                        });
                    }
                })
            })
        } else {
            let confirming_delete = self.confirming_delete.clone();
            let backend_handle = self.backend_handle.clone();
            Button::new(("delete", element_id))
                .danger()
                .icon(PandoraIcon::Trash2)
                .on_click(cx.listener(move |this, click: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    let delegate = this.delegate();

                    // If quick_delete_mods is enabled and shift clicking, delete instantly
                    if InterfaceConfig::get(cx).quick_delete_mods && click.modifiers().shift {
                        if delegate.is_selected(element_id) {
                            let content_ids = delegate
                                .content
                                .iter()
                                .filter_map(|summary| delegate.is_selected(summary.filename_hash).then(|| summary.id))
                                .collect();

                            backend_handle.send(MessageToBackend::DeleteContent { id, content_ids });
                        } else {
                            backend_handle.send(MessageToBackend::DeleteContent {
                                id,
                                content_ids: vec![content_id],
                            });
                        }
                        return;
                    }

                    let mut confirming_delete = confirming_delete.lock();
                    confirming_delete.clear();
                    if delegate.is_selected(element_id) {
                        confirming_delete.extend(&delegate.selected);
                        confirming_delete.extend(&delegate.selected_range);
                    } else {
                        confirming_delete.insert(element_id);
                    }
                }))
        };

        let status = summary.update.status_if_matches(self.for_loader, self.for_version.as_str());
        let is_loading = self.updating.lock().contains(&element_id);
        let open_change_version = {
            let summary = summary.clone();
            let data = self.data.clone();
            let updating = self.updating.clone();
            let list = cx.entity();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                crate::modals::change_version::open(id, &summary, &data, updating.clone(), list.clone(), window, cx);
            }
        };
        let source_name = match summary.content_source {
            ContentSource::ModrinthProject { .. } | ContentSource::ModrinthUnknown => t::modrinth::name(),
            ContentSource::CurseforgeProject { .. } => t::curseforge::name(),
            _ => t::common::unknown(),
        };
        let update_button = match status {
            bridge::instance::ContentUpdateStatus::Unknown | bridge::instance::ContentUpdateStatus::ManualInstall => {
                if summary.content_source == ContentSource::Manual {
                    Some(
                        Button::new(("update", element_id))
                            .warning()
                            .icon(PandoraIcon::FileQuestionMark)
                            .tooltip(t::instance::content::change_version::installed_manually()),
                    )
                } else {
                    Some(
                        Button::new(("update", element_id))
                            .icon(PandoraIcon::ArrowLeftRight)
                            .loading(is_loading)
                            .tooltip(t::instance::content::change_version::button(source_name))
                            .on_click(open_change_version.clone()),
                    )
                }
            },
            bridge::instance::ContentUpdateStatus::ErrorNotFound => Some(
                Button::new(("update", element_id))
                    .danger()
                    .icon(PandoraIcon::TriangleAlert)
                    .loading(is_loading)
                    .tooltip(t::instance::content::change_version::no_compatible_versions())
                    .on_click(open_change_version.clone()),
            ),
            bridge::instance::ContentUpdateStatus::ErrorInvalidHash => Some(
                Button::new(("update", element_id))
                    .danger()
                    .icon(PandoraIcon::TriangleAlert)
                    .loading(is_loading)
                    .tooltip(t::instance::content::update::check::invalid_hash_error())
                    .on_click(open_change_version.clone()),
            ),
            bridge::instance::ContentUpdateStatus::AlreadyUpToDate => Some(
                Button::new(("update", element_id))
                    .icon(PandoraIcon::ArrowLeftRight)
                    .loading(is_loading)
                    .tooltip(t::instance::content::change_version::up_to_date())
                    .on_click(open_change_version.clone()),
            ),
            bridge::instance::ContentUpdateStatus::Modrinth | bridge::instance::ContentUpdateStatus::Curseforge => {
                let updating = self.updating.clone();
                let backend_handle = self.backend_handle.clone();
                Some(
                    Button::new(("update", element_id))
                        .success()
                        .icon(PandoraIcon::Download)
                        .loading(is_loading)
                        .tooltip(t::instance::content::change_version::update_available(source_name))
                        .on_click(cx.listener(move |this, click: &ClickEvent, window, cx| {
                            cx.stop_propagation();

                            if !click.modifiers().shift {
                                open_change_version(click, window, cx);
                                return;
                            }

                            let delegate = this.delegate_mut();
                            if delegate.is_selected(element_id) {
                                for summary in &delegate.content {
                                    if delegate.is_selected(summary.filename_hash)
                                        && summary.update.can_update(delegate.for_loader, delegate.for_version.as_str())
                                    {
                                        crate::root::update_single_mod(
                                            id,
                                            summary.id,
                                            summary.filename_hash,
                                            &updating,
                                            &backend_handle,
                                            window,
                                            cx,
                                        );
                                    }
                                }
                                delegate.selected.clear();
                                delegate.selected_range.clear();
                                delegate.last_clicked_non_range = None;
                            } else {
                                crate::root::update_single_mod(
                                    id,
                                    content_id,
                                    element_id,
                                    &updating,
                                    &backend_handle,
                                    window,
                                    cx,
                                );
                            }
                            cx.notify();
                        })),
                )
            },
        };

        let unzip_button = if summary.content_summary.extra.modpack_files().is_some() {
            Some(
                Button::new(("unzip", element_id))
                    .outline()
                    .icon(PandoraIcon::PackageOpen)
                    .tooltip(t::instance::content::unzip::tooltip())
                    .on_click({
                        let instance_id = self.id;
                        let content_id = summary.id;
                        let content_title = summary.filename.clone();
                        let backend_handle = self.backend_handle.clone();
                        move |_: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            crate::modals::unzip_modpack::open_unzip_modpack(
                                instance_id,
                                content_id,
                                &content_title,
                                backend_handle.clone(),
                                window,
                                cx,
                            );
                        }
                    }),
            )
        } else {
            None
        };

        let backend_handle = self.backend_handle.clone();

        let toggle_control = Switch::new(("toggle", element_id))
            .checked(summary.enabled)
            .disabled(!summary.can_toggle)
            .when(compact, |this| this.small())
            .when(summary.can_toggle, |this| {
                this.on_click(cx.listener(move |this, checked, window, cx| {
                    cx.stop_propagation();
                    window.prevent_default();

                    let delegate = this.delegate();
                    if delegate.is_selected(element_id) {
                        let content_ids = delegate
                            .content
                            .iter()
                            .filter_map(|summary| {
                                if delegate.is_selected(summary.filename_hash) {
                                    Some(summary.id)
                                } else {
                                    None
                                }
                            })
                            .collect();

                        backend_handle.send(MessageToBackend::SetContentEnabled {
                            id,
                            content_ids,
                            enabled: *checked,
                        });
                    } else {
                        backend_handle.send(MessageToBackend::SetContentEnabled {
                            id,
                            content_ids: vec![content_id],
                            enabled: *checked,
                        });
                    }
                }))
            })
            .px_2();

        let expand_control = expand_index.map(|expand_index| {
            let expand_icon = if self.expanded.load(Ordering::Relaxed) == expand_index {
                PandoraIcon::ArrowDown
            } else {
                PandoraIcon::ArrowRight
            };

            Button::new(("expand", element_id)).icon(expand_icon).compact().small().info().on_click({
                let expanded = self.expanded.clone();
                move |_, _, _| {
                    let value = expanded.load(Ordering::Relaxed);
                    if value == expand_index {
                        expanded.store(0, Ordering::Relaxed);
                    } else {
                        expanded.store(expand_index, Ordering::Relaxed);
                    }
                }
            })
        });

        let controls = if compact {
            // The expand button sits beside the toggle rather than under it, keeping the row one line
            // tall. Rows that can't expand keep its space, so every filename starts in the same place.
            let expand_slot = match expand_control {
                Some(expand_control) => Some(expand_control.into_any_element()),
                None if self.searched.is_none() && self.has_expandable_rows => {
                    Some(div().w_6().flex_none().into_any_element())
                },
                None => None,
            };
            h_flex().gap_1().child(toggle_control).children(expand_slot).into_any_element()
        } else if let Some(expand_control) = expand_control {
            v_flex()
                .items_center()
                .gap_1()
                .child(toggle_control)
                .child(expand_control)
                .into_any_element()
        } else {
            toggle_control.into_any_element()
        };

        let mut right_buttons = Vec::new();
        if let Some(category) = &summary.label.category {
            right_buttons.push(content_label::category_chip(category, category_color, cx).into_any_element());
        }
        if let Some(unzip_button) = unzip_button {
            right_buttons.push(unzip_button.with_size(button_size).into_any_element());
        }
        if let Some(update_button) = update_button {
            right_buttons.push(update_button.with_size(button_size).into_any_element());
        }
        right_buttons.push(delete_button.with_size(button_size).into_any_element());

        let item_content = if compact {
            // The filename carries the version, so it's all of the item a compact row shows
            let filename = SharedString::from(display_filename(&summary.filename).to_string());

            h_flex()
                .id(("row", element_id))
                .w_full()
                .gap_1()
                .child(controls)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pl_1()
                        .truncate()
                        .when(!summary.enabled, |this| this.line_through())
                        .child(filename),
                )
                .child(h_flex().flex_none().gap_1().pr_3().children(right_buttons))
        } else {
            let icon = display.show_icons.then(|| {
                let icon = if let Some(png_icon) = summary.content_summary.png_icon.as_ref() {
                    png_render_cache::render(png_icon.clone(), cx)
                } else {
                    png_render_cache::embedded_image("images/default_mod.png")
                };
                icon.size_16().min_w_16().min_h_16().grayscale(!summary.enabled)
            });

            let (desc1, desc2) = create_descriptions(
                summary.content_summary.name.clone(),
                summary.content_summary.version_str.clone(),
                summary.content_summary.authors.clone(),
                summary.content_summary.rich_description.clone(),
                !summary.enabled,
                summary.filename.clone(),
                cx.theme().muted_foreground,
            );

            h_flex()
                .id(("row", element_id))
                .gap_1()
                .child(controls)
                .children(icon)
                .when(!summary.enabled, |this| this.line_through())
                .line_height(rems(1.2))
                .child(desc1.pl_2())
                .when_some(desc2, |div, desc2| div.child(desc2))
                .child(h_flex().absolute().right_4().gap_2().children(right_buttons))
        };

        let item_content = item_content
            .border_1()
            .map(|content| match tint {
                Some(color) => {
                    let (background, border) = content_label::row_tint(color, selected, cx);
                    content.rounded(cx.theme().radius).bg(background).border_color(border)
                },
                None => content.when(selected, |content| {
                    content.border_color(cx.theme().selection).bg(cx.theme().selection.alpha(0.2))
                }),
            })
            .context_menu({
                let list = cx.entity();
                move |menu, window, cx| match list.read(cx).delegate().label_menu_for(element_id) {
                    Some(label_menu) => Rc::new(label_menu).build(menu, window, cx),
                    None => menu,
                }
            });

        ListItem::new(("item", element_id))
            .map(|item| if compact { item.px_1().py_0p5() } else { item.p_1() })
            .child(item_content)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    // Right-clicking outside the selection labels that row alone, so it becomes the
                    // selection to show what the menu will act on
                    let delegate = this.delegate_mut();
                    if !delegate.is_selected(element_id) {
                        delegate.confirming_delete.lock().clear();
                        delegate.selected_range.clear();
                        delegate.selected.clear();
                        delegate.selected.insert(element_id);
                        delegate.last_clicked_non_range = Some(element_id);
                        cx.notify();
                    }
                }),
            )
            .on_click(cx.listener(move |this, click: &ClickEvent, _, cx| {
                cx.stop_propagation();
                if click.standard_click() {
                    let delegate = this.delegate_mut();
                    delegate.confirming_delete.lock().clear();
                    if click.modifiers().shift
                        && let Some(from) = delegate.last_clicked_non_range
                    {
                        delegate.selected_range.clear();

                        if let Some(searched) = &delegate.searched {
                            let from_index = searched.iter().position(|element| match element {
                                SummaryOrChild::Summary(summary) => summary.filename_hash == from,
                                SummaryOrChild::Child(_) => false,
                            });

                            let Some(from_index) = from_index else {
                                return;
                            };

                            let to_index = searched.iter().position(|element| match element {
                                SummaryOrChild::Summary(summary) => summary.filename_hash == element_id,
                                SummaryOrChild::Child(_) => false,
                            });

                            let Some(to_index) = to_index else {
                                return;
                            };

                            let min_index = from_index.min(to_index);
                            let max_index = from_index.max(to_index);

                            for add in searched[min_index..=max_index].iter() {
                                match add {
                                    SummaryOrChild::Summary(summary) => {
                                        delegate.selected_range.insert(summary.filename_hash);
                                    },
                                    SummaryOrChild::Child(_) => {},
                                }
                            }
                        } else {
                            let from_index = delegate.content.iter().position(|element| element.filename_hash == from);

                            let Some(from_index) = from_index else {
                                return;
                            };

                            let to_index =
                                delegate.content.iter().position(|element| element.filename_hash == element_id);

                            let Some(to_index) = to_index else {
                                return;
                            };

                            let min_index = from_index.min(to_index);
                            let max_index = from_index.max(to_index);

                            for add in delegate.content[min_index..=max_index].iter() {
                                delegate.selected_range.insert(add.filename_hash);
                            }
                        }
                    } else if click.modifiers().secondary() || click.modifiers().shift {
                        // Cmd+Click (macos), Ctrl+Click (win/linux)

                        delegate.selected.extend(&delegate.selected_range);
                        delegate.selected_range.clear();

                        if delegate.selected.contains(&element_id) {
                            delegate.selected.remove(&element_id);
                        } else {
                            delegate.selected.insert(element_id);
                        }

                        delegate.last_clicked_non_range = Some(element_id);
                    } else {
                        delegate.selected_range.clear();
                        delegate.selected.clear();
                        delegate.selected.insert(element_id);
                        delegate.last_clicked_non_range = Some(element_id);
                    }
                }
            }))
    }

    fn render_child_entry(&self, child: &ContentEntryChild, cx: &mut Context<ListState<Self>>) -> ListItem {
        let display = self.display(cx);
        let compact = display.is_compact();
        let button_size = if compact { Size::Small } else { Size::Medium };
        let summary = &child.summary;

        let mut hasher = DefaultHasher::new();
        child.parent_filename_hash.hash(&mut hasher);
        child.path.hash(&mut hasher);
        let element_id = hasher.finish();

        let enabled = child.enabled;
        // A blocked third-party download only matters while the file is actually missing. Once it's
        // present in the library (e.g. recovered from Modrinth), it's a normal, toggleable mod.
        let blocked = child.disabled_third_party_downloads && child.is_missing;
        let visually_enabled = enabled && child.parent_enabled && !blocked;

        let (tint, category_color) = {
            let categories = &InterfaceConfig::get(cx).content_categories;
            (
                content_label::resolve_color(&child.label, categories),
                content_label::category_color(child.label.category.as_deref(), categories),
            )
        };

        let toggle_control = Switch::new(("toggle", element_id))
            .checked(enabled && !blocked)
            .when(compact, |this| this.small())
            .when_else(
                child.is_missing,
                |this| this.disabled(true),
                |this| {
                    this.on_click({
                        let id = self.id;
                        let content_id = child.parent;
                        let child_id = child.summary.id.clone();
                        let child_name = child.summary.name.clone();
                        let path = child.path.clone();
                        let disabled_default = child.disabled_default;
                        let backend_handle = self.backend_handle.clone();
                        move |checked, window, cx| {
                            cx.stop_propagation();
                            window.prevent_default();

                            backend_handle.send(MessageToBackend::SetContentChildEnabled {
                                id,
                                content_id,
                                child_id: child_id.clone(),
                                child_name: child_name.clone(),
                                child_filename: path.clone(),
                                disabled_default,
                                enabled: *checked,
                            });
                        }
                    })
                },
            )
            .px_2();

        let mut item_content = h_flex().id(("row", element_id)).gap_1().pl_4().child(toggle_control);

        if compact {
            item_content = item_content.w_full().child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pl_1()
                    .truncate()
                    .when(!visually_enabled, |this| this.line_through())
                    .child(SharedString::from(child.path.clone())),
            );
        } else {
            let icon = display.show_icons.then(|| {
                let icon = if let Some(png_icon) = summary.png_icon.as_ref() {
                    png_render_cache::render(png_icon.clone(), cx)
                } else {
                    png_render_cache::embedded_image("images/default_mod.png")
                };
                icon.size_16().min_w_16().min_h_16().grayscale(!visually_enabled)
            });

            let (desc1, desc2) = create_descriptions(
                summary.name.clone(),
                summary.version_str.clone(),
                summary.authors.clone(),
                summary.rich_description.clone(),
                !visually_enabled,
                child.path.clone(),
                cx.theme().muted_foreground,
            );

            item_content = item_content
                .children(icon)
                .line_height(rems(1.2))
                .child(desc1.pl_2().when(!visually_enabled, |this| this.line_through()))
                .when_some(desc2, |div, desc2| div.child(desc2.when(!visually_enabled, |this| this.line_through())));
        }

        let category_chip = child
            .label
            .category
            .as_deref()
            .map(|category| content_label::category_chip(category, category_color, cx));

        if child.disabled_third_party_downloads {
            let download_button = Button::new("download")
                .label(t::instance::content::download())
                .success()
                .with_size(button_size)
                .on_click({
                    let backend_handle = self.backend_handle.clone();
                    let id = self.id;
                    let content_id = child.parent;
                    move |_, window, cx| {
                        let modal_action = ModalAction::default();

                        backend_handle.send(MessageToBackend::DownloadContentChildren {
                            id,
                            content_id,
                            modal_action: modal_action.clone(),
                        });

                        crate::modals::generic::show_modal(
                            window,
                            cx,
                            t::instance::content::downloading_children().into(),
                            t::instance::content::error_downloading_children().into(),
                            modal_action,
                        );
                    }
                });

            if compact {
                // The alert is taller than a compact row, so its text moves to a warning button's tooltip
                let blocked_message = format!(
                    "{}: {}",
                    t::instance::content::blocked(),
                    t::instance::content::install::no_third_party_downloads()
                );
                item_content = item_content.child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .pr_3()
                        .children(category_chip)
                        .child(
                            Button::new(("blocked", element_id))
                                .warning()
                                .icon(PandoraIcon::TriangleAlert)
                                .with_size(button_size)
                                .tooltip(blocked_message),
                        )
                        .child(download_button),
                );
            } else {
                item_content = item_content
                    .children(category_chip)
                    .child(
                        ErrorAlert::new(
                            t::instance::content::blocked().into(),
                            t::instance::content::install::no_third_party_downloads().into(),
                        )
                        .w(Length::Auto),
                    )
                    .child(download_button);
            }
        } else {
            let id = self.id;
            let content_id = child.parent;
            let child_id = child.summary.id.clone();
            let child_name = child.summary.name.clone();
            let child_filename = child.path.clone();
            let disabled_default = child.disabled_default;

            let delete_button = if self.confirming_delete_children.lock().contains(&element_id) {
                let backend_handle = self.backend_handle.clone();
                Button::new(("delete-child", element_id))
                    .danger()
                    .icon(PandoraIcon::Check)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delegate().confirming_delete_children.lock().clear();
                        backend_handle.send(MessageToBackend::SetContentChildEnabled {
                            id,
                            content_id,
                            child_id: child_id.clone(),
                            child_name: child_name.clone(),
                            child_filename: child_filename.clone(),
                            disabled_default,
                            enabled: false,
                        });
                    }))
            } else {
                let confirming_delete_children = self.confirming_delete_children.clone();
                let backend_handle = self.backend_handle.clone();
                Button::new(("delete-child", element_id))
                    .danger()
                    .icon(PandoraIcon::Trash2)
                    .on_click(cx.listener(move |this, click: &ClickEvent, _, cx| {
                        cx.stop_propagation();

                        if InterfaceConfig::get(cx).quick_delete_mods && click.modifiers().shift {
                            backend_handle.send(MessageToBackend::SetContentChildEnabled {
                                id,
                                content_id,
                                child_id: child_id.clone(),
                                child_name: child_name.clone(),
                                child_filename: child_filename.clone(),
                                disabled_default,
                                enabled: false,
                            });
                            return;
                        }

                        let mut guard = confirming_delete_children.lock();
                        guard.clear();
                        guard.insert(element_id);
                        this.delegate().confirming_delete.lock().clear();
                    }))
            }
            .with_size(button_size);

            if compact {
                item_content = item_content
                    .child(h_flex().flex_none().gap_1().pr_3().children(category_chip).child(delete_button));
            } else {
                item_content = item_content
                    .child(h_flex().absolute().right_4().gap_2().children(category_chip).child(delete_button));
            }
        }

        let label_menu = Rc::new(LabelMenu {
            target: LabelTarget::Child {
                instance: self.id,
                content_id: child.parent,
                child_keys: child.label_keys.clone(),
            },
            labels: vec![child.label.clone()],
            preview_name: SharedString::from(child.path.clone()),
            backend_handle: self.backend_handle.clone(),
        });

        let item_content = item_content
            .when_some(tint, |content, color| {
                let (background, border) = content_label::row_tint(color, false, cx);
                content.border_1().rounded(cx.theme().radius).bg(background).border_color(border)
            })
            .context_menu(move |menu, window, cx| label_menu.build(menu, window, cx));

        ListItem::new(("item", element_id))
            .map(|item| if compact { item.px_1().py_0p5() } else { item.p_1() })
            .child(item_content)
    }

    pub fn global_labels_changed(&self, cx: &App) -> bool {
        self.global_labels != InterfaceConfig::get(cx).global_content_labels
    }

    pub fn set_content(&mut self, new_content: &[InstanceContentSummary], cx: &App) {
        let last_mods_len = self.content.len();
        self.global_labels = InterfaceConfig::get(cx).global_content_labels.clone();
        let resolve = |label: ContentLabel, keys: &[Arc<str>]| {
            label.resolve(
                &find_global_label(&self.global_labels, keys)
                    .map(|rule| rule.label.clone())
                    .unwrap_or_default(),
            )
        };

        struct Item {
            modification: InstanceContentSummary,
            children: Vec<ContentEntryChild>,
        }

        let mut items = Vec::with_capacity(new_content.len());

        for modification in new_content.iter() {
            let mut inner_children = Vec::new();

            let extra = &modification.content_summary.extra;
            let files = if let ContentType::ModrinthModpack { files, .. } = extra {
                Some(files)
            } else if let ContentType::CurseforgeModpack {
                unknown_files, files, ..
            } = &extra
            {
                // Not-yet-downloaded files have an unknown destination folder, so only surface
                // them under the Mods tab (where the modpack bundle itself lives).
                if self.content_folder == ContentFolder::Mods {
                    for unknown_file in unknown_files.iter() {
                        let filename: Arc<str> = t::instance::content::file_id(unknown_file.file_id).into();

                        let lowercase_filename: Arc<str> = filename.to_ascii_lowercase().into();
                        let lowercase_search_keys = Arc::new([lowercase_filename]);

                        // Keyed by the file id rather than the shown name, which is translated
                        let label_keys: Arc<[Arc<str>]> =
                            child_label_keys(None, None, &format!("curseforge/{}", unknown_file.file_id)).into();
                        let label =
                            find_child_label(&modification.child_labels, &label_keys).cloned().unwrap_or_default();
                        let keys = global_content_keys(
                            self.content_folder,
                            &ContentSource::CurseforgeProject {
                                project_id: unknown_file.project_id,
                            },
                            None,
                            &filename,
                        );
                        let label = resolve(label, &keys);

                        inner_children.push(ContentEntryChild {
                            summary: UNKNOWN_CONTENT_SUMMARY.clone(),
                            parent_filename_hash: modification.filename_hash,
                            parent: modification.id,
                            lowercase_search_keys,
                            path: filename,
                            filesize: 0,
                            disabled_default: false,
                            enabled: true,
                            parent_enabled: modification.enabled,
                            disabled_third_party_downloads: false,
                            is_missing: true,
                            label,
                            label_keys,
                        });
                    }
                }

                Some(files)
            } else {
                None
            };

            if let Some(files) = files {
                for file in files.iter() {
                    if modification.disabled_children.deleted_filenames.contains(file.path.as_str()) {
                        continue;
                    }

                    // Only show a modpack's children under the tab matching their destination
                    // folder (mods → Mods, resourcepacks → Resourcepacks, shaderpacks → Shaders).
                    match file.path() {
                        Some(path) => {
                            if !path.starts_with(self.content_folder.folder_name()) {
                                continue;
                            }
                        },
                        None => {
                            // Destination unknown (not downloaded yet) — only under Mods.
                            if self.content_folder != ContentFolder::Mods {
                                continue;
                            }
                        },
                    }

                    let summary = file.summary.clone();

                    let mut id = None;
                    let mut name = None;

                    if let Some(content_summary) = &summary {
                        id = content_summary.id.as_ref().map(|s| &**s);
                        name = content_summary.name.as_ref().map(|s| &**s);
                    }

                    let enabled =
                        modification
                            .disabled_children
                            .is_enabled(file.default_disabled, id, name, file.path.as_str());

                    let label_keys: Arc<[Arc<str>]> = child_label_keys(id, name, file.path.as_str()).into();
                    let label = find_child_label(&modification.child_labels, &label_keys).cloned().unwrap_or_default();
                    let keys = global_content_keys(
                        self.content_folder,
                        &modpack_file_source(&modification.content_summary, file),
                        summary.as_deref(),
                        file.path.as_str(),
                    );
                    let label = resolve(label, &keys);

                    let is_missing = summary.is_none();
                    let summary = summary.unwrap_or(UNKNOWN_CONTENT_SUMMARY.clone());

                    let lowercase_filename: Arc<str> = file.path.as_str().to_ascii_lowercase().into();

                    let lowercase_search_keys = summary
                        .id
                        .clone()
                        .into_iter()
                        .chain(summary.name.clone().into_iter())
                        .chain(std::iter::once(lowercase_filename))
                        .collect();

                    let filesize = match &file.source {
                        bridge::instance::ModpackFileSource::DownloadUrl { size, .. } => *size as u64,
                        bridge::instance::ModpackFileSource::DownloadCurseforge { .. } => 0,
                        bridge::instance::ModpackFileSource::Builtin { bytes } => bytes.len() as u64,
                    };

                    inner_children.push(ContentEntryChild {
                        summary,
                        parent_filename_hash: modification.filename_hash,
                        parent: modification.id,
                        lowercase_search_keys,
                        path: file.path.as_str().into(),
                        filesize,
                        disabled_default: file.default_disabled,
                        enabled,
                        parent_enabled: modification.enabled,
                        disabled_third_party_downloads: file.disabled_third_party_downloads,
                        is_missing,
                        label,
                        label_keys,
                    });
                }
            }

            // In a non-Mods tab, a modpack bundle (which lives in the mods folder) is only shown
            // when it actually contributes files to this folder; otherwise it's just clutter.
            if self.content_folder != ContentFolder::Mods
                && modification.content_summary.extra.is_modpack()
                && inner_children.is_empty()
            {
                continue;
            }

            let mut modification = modification.clone();
            let keys = global_content_keys(
                self.content_folder,
                &modification.content_source,
                Some(&modification.content_summary),
                &modification.filename,
            );
            modification.label = resolve(modification.label, &keys);
            items.push(Item {
                modification,
                children: inner_children,
            });
        }

        let enabled_first = self.enabled_first;
        let sort_key = self.sort_key;

        items.sort_by(|a, b| {
            let ordering = a.children.len().cmp(&b.children.len()).reverse();
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }

            if enabled_first && a.modification.enabled != b.modification.enabled {
                return b.modification.enabled.cmp(&a.modification.enabled);
            }

            let ordering = sort_key.compare(&a.modification, &b.modification);
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }

            for key in InstanceContentSortKey::iter() {
                if key == sort_key {
                    continue;
                }

                let ordering = key.compare(&a.modification, &b.modification);
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }

            return std::cmp::Ordering::Equal;
        });

        for item in &mut items {
            item.children.sort_by(|a, b| {
                if enabled_first && a.enabled != b.enabled {
                    return b.enabled.cmp(&a.enabled);
                }

                let ordering = a.compare_by(b, sort_key);
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }

                for key in InstanceContentSortKey::iter() {
                    if key == sort_key {
                        continue;
                    }

                    let ordering = a.compare_by(b, key);
                    if ordering != std::cmp::Ordering::Equal {
                        return ordering;
                    }
                }

                return std::cmp::Ordering::Equal;
            });
        }

        let mods: Vec<_> = items.iter().map(|i| i.modification.clone()).collect();
        let children: Vec<_> = items.into_iter().map(|i| i.children).collect();

        let mut updating = self.updating.lock();
        if !updating.is_empty() {
            let ids: FxHashSet<u64> = mods.iter().map(|summary| summary.filename_hash).collect();
            updating.retain(|id| ids.contains(&id));
        }
        drop(updating);

        self.loading = false;
        self.content = mods.clone();
        self.has_expandable_rows = children.iter().any(|children| !children.is_empty());
        self.children = children;
        self.searched = None;
        self.confirming_delete.lock().clear();
        self.confirming_delete_children.lock().clear();
        if last_mods_len != self.content.len() {
            self.expanded.store(0, Ordering::Release);
        }
        let _ = self.actual_perform_search(&self.last_query.clone());
    }

    fn actual_perform_search(&mut self, query: &str) {
        let query = query.trim_ascii();

        self.last_clicked_non_range = None;

        if query.is_empty() {
            self.last_query = SharedString::new_static("");
            self.searched = None;
            return;
        }

        self.last_query = SharedString::new(query);

        let query = query.to_lowercase();

        let mut searched = Vec::new();

        for (m, children) in self.content.iter().zip(self.children.iter()) {
            let mut parent_added = false;

            if m.lowercase_search_keys.iter().any(|f| f.contains(&query)) || category_matches(&m.label, &query) {
                parent_added = true;
                searched.push(SummaryOrChild::Summary(m.clone()));
            }

            for child in children {
                if child.lowercase_search_keys.iter().any(|f| f.contains(&query))
                    || category_matches(&child.label, &query)
                {
                    if !parent_added {
                        parent_added = true;
                        searched.push(SummaryOrChild::Summary(m.clone()));
                    }

                    searched.push(SummaryOrChild::Child(child.clone()));
                }
            }
        }

        self.searched = Some(searched);
    }

    fn is_selected(&self, element_id: u64) -> bool {
        self.selected.contains(&element_id) || self.selected_range.contains(&element_id)
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.selected_range.clear();
        self.last_clicked_non_range = None;
        self.confirming_delete.lock().clear();
        self.confirming_delete_children.lock().clear();
    }

    pub fn select_all(&mut self) {
        self.clear_selection();

        if let Some(searched) = &self.searched {
            for element in searched {
                if let SummaryOrChild::Summary(summary) = element {
                    self.selected.insert(summary.filename_hash);
                }
            }
        } else {
            for summary in &self.content {
                self.selected.insert(summary.filename_hash);
            }
        }
    }
}

impl ListDelegate for ContentListDelegate {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        if let Some(searched) = &self.searched {
            return searched.len();
        }

        let expanded = self.expanded.load(Ordering::Relaxed);
        if expanded > 0 {
            self.content.len() + self.children[expanded - 1].len()
        } else {
            self.content.len()
        }
    }

    fn loading(&self, _cx: &App) -> bool {
        self.loading
    }

    fn render_loading(&mut self, _window: &mut Window, cx: &mut Context<ListState<Self>>) -> impl IntoElement {
        v_flex()
            .w_full()
            .h_1_2()
            .items_center()
            .justify_center()
            .child(Spinner::new().color(cx.theme().muted_foreground).with_size(px(36.0)))
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let mut index = ix.row;

        if let Some(searched) = &self.searched {
            let item = searched.get(index)?;
            match item {
                SummaryOrChild::Summary(instance_mod_summary) => {
                    let selected = self.is_selected(instance_mod_summary.filename_hash);
                    return Some(self.render_summary(instance_mod_summary, selected, None, cx));
                },
                SummaryOrChild::Child(mod_entry_child) => {
                    return Some(self.render_child_entry(mod_entry_child, cx));
                },
            }
        }

        let expanded = self.expanded.load(Ordering::Relaxed);

        if expanded > 0 && index >= expanded {
            if let Some(child) = self.children[expanded - 1].get(index - expanded) {
                return Some(self.render_child_entry(child, cx));
            }
            index -= self.children[expanded - 1].len();
        }

        let summary = self.content.get(index)?;
        let selected = self.is_selected(summary.filename_hash);

        let expand_index = if self.children[index].is_empty() {
            None
        } else {
            Some(index + 1)
        };
        Some(self.render_summary(summary, selected, expand_index, cx))
    }

    fn set_selected_index(&mut self, _ix: Option<IndexPath>, _window: &mut Window, _cx: &mut Context<ListState<Self>>) {
    }

    fn perform_search(&mut self, query: &str, _window: &mut Window, _cx: &mut Context<ListState<Self>>) -> Task<()> {
        self.actual_perform_search(query);
        Task::ready(())
    }
}

/// The filename without the `.disabled` a disabled item's file has; the toggle already says that.
fn display_filename(filename: &str) -> &str {
    filename.strip_suffix(".disabled").unwrap_or(filename)
}

/// Whether the search matches the item's category, by its id or the name it's shown as.
fn category_matches(label: &ContentLabel, query: &str) -> bool {
    label.category.as_deref().is_some_and(|category| {
        category.to_lowercase().contains(query) || content_label::category_name(category).to_lowercase().contains(query)
    })
}

fn create_descriptions(
    name: Option<Arc<str>>,
    version: Arc<str>,
    authors: Arc<str>,
    rich_description: Option<Arc<FlatTextComponent>>,
    grayscale: bool,
    filename: Arc<str>,
    secondary: Hsla,
) -> (Div, Option<Div>) {
    if name.is_none() && authors.is_empty() {
        if let Some(rich_description) = rich_description {
            let styled_text = super::create_styled_text(&*rich_description, grayscale);

            let description1 = v_flex()
                .min_w_2_5()
                .gap_2()
                .whitespace_nowrap()
                .overflow_x_hidden()
                .line_height(relative(1.0))
                .child(SharedString::from(filename))
                .child(div().line_clamp(2).child(styled_text));
            return (description1, None);
        }

        let description1 = v_flex()
            .min_w_1_5()
            .whitespace_nowrap()
            .overflow_x_hidden()
            .child(SharedString::from(filename))
            .child(div().text_color(secondary).child(SharedString::from(version)));
        return (description1, None);
    }

    let description1 = v_flex()
        .min_w_1_5()
        .whitespace_nowrap()
        .overflow_x_hidden()
        .child(SharedString::from(name.clone().unwrap_or(filename.clone())))
        .child(div().text_color(secondary).child(SharedString::from(version)));

    let mut description2 = v_flex().text_color(secondary).child(SharedString::from(authors));

    if name.is_some() {
        description2 = description2.child(SharedString::from(filename));
    }

    (description1, Some(description2))
}
