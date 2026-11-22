//! Files dropped onto the launcher from outside it.
//!
//! One file gets a dialog: what it is and which project it's a build of, found by its hash on
//! Modrinth or CurseForge or given by hand, and where it should go. Several files dropped onto an
//! instance's or server's content go straight in; dropped anywhere else they only ask where to go.
//!
//! Also the dialog for linking content added by hand to its project afterwards.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use bridge::{
    dropped::{DropTarget, DroppedFile, DroppedFileInfo, ResolvedProject},
    handle::BackendHandle,
    instance::{ContentFolder, InstanceID},
    message::MessageToBackend,
    meta::MetadataRequest,
    modal_action::ModalAction,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, ElementExt, IndexPath, Selectable, Sizable, ThemeStyled, WindowExt,
    button::{Button, ButtonGroup, ButtonVariants},
    checkbox::Checkbox,
    dialog::Dialog,
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement,
    select::{SearchableVec, Select, SelectState},
    spinner::Spinner,
    v_flex,
};
use schema::{
    content::ContentSource, loader::Loader, server::ServerPlatform, version_manifest::MinecraftVersionManifest,
    version_manifest::MinecraftVersionType,
};
use strum::IntoEnumIterator;

use crate::{
    component::named_dropdown::{DropdownName, NamedDropdown, NamedDropdownItem},
    entity::{
        DataEntities,
        metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult},
    },
    icon::PandoraIcon,
    png_render_cache,
};

/// Files dropped somewhere in the launcher.
pub struct DropRequest {
    pub files: Vec<PathBuf>,
    /// The instance or server whose content they were dropped on, which then needs no choosing.
    pub target: Option<InstanceID>,
    /// Where a file whose type can't be told goes: the tab it was dropped on.
    pub fallback_folder: Option<ContentFolder>,
}

/// An already-installed file in an instance/server whose filename matches a dragged file.
pub struct ExistingMatch {
    pub folder: ContentFolder,
    pub filename: SharedString,
    pub path: Arc<Path>,
    pub source: ContentSource,
}

fn filename_matches(existing: &str, dropped: &str) -> bool {
    let existing = existing.strip_suffix(".disabled").unwrap_or(existing);
    #[cfg(windows)]
    {
        existing.eq_ignore_ascii_case(dropped)
    }
    #[cfg(not(windows))]
    {
        existing == dropped
    }
}

/// Checks whether `dropped_path` has the same filename as an already-installed content file in
/// `instance_id` (searching `preferred_folder` first, then the remaining content folders).
pub fn find_matching_installed_file(
    instance_id: InstanceID,
    preferred_folder: Option<ContentFolder>,
    dropped_path: &Path,
    data: &DataEntities,
    cx: &App,
) -> Option<ExistingMatch> {
    let dropped_name = dropped_path.file_name()?.to_string_lossy();
    let entry = data.instances.read(cx).entries.get(&instance_id)?.clone();
    let instance = entry.read(cx);

    let mut folders: Vec<ContentFolder> = Vec::with_capacity(5);
    if let Some(preferred) = preferred_folder {
        folders.push(preferred);
    }
    for folder in [
        ContentFolder::Mods,
        ContentFolder::Plugins,
        ContentFolder::ResourcePacks,
        ContentFolder::Shaders,
        ContentFolder::Datapacks,
    ] {
        if !folders.contains(&folder) {
            folders.push(folder);
        }
    }

    for folder in folders {
        let Some(items) = instance.content[folder].read(cx).as_ref() else {
            continue;
        };
        if let Some(summary) = items.iter().find(|item| filename_matches(&item.filename, &dropped_name)) {
            let source = match &summary.content_source {
                s @ (ContentSource::ModrinthProject { .. } | ContentSource::CurseforgeProject { .. }) => s.clone(),
                _ => ContentSource::Manual,
            };
            return Some(ExistingMatch {
                folder,
                filename: SharedString::from(dropped_name.into_owned()),
                path: summary.path.clone(),
                source,
            });
        }
    }
    None
}

pub fn handle_drop(request: DropRequest, data: &DataEntities, window: &mut Window, cx: &mut App) {
    if request.files.is_empty() {
        return;
    }
    if let Some(target) = request.target {
        // Dropping a single file onto an instance/server whose content already has a file with the
        // same filename replaces it in place, skips the modal, and keeps its selected project.
        if request.files.len() == 1
            && let Some(existing) =
                find_matching_installed_file(target, request.fallback_folder, &request.files[0], data, cx)
        {
            let files: Arc<[DroppedFile]> = Arc::from([DroppedFile {
                path: request.files[0].clone().into(),
                source: existing.source,
                fallback_folder: Some(existing.folder),
                replace_old: Some(existing.path),
            }]);
            install(files, DropTarget::Instance(target), &data.backend_handle, window, cx);
            return;
        }

        // Several files dropped onto an instance's own content need nothing asked: each goes where
        // its type says, and gets linked to its project once the folder reloads (keeping any
        // existing project if a file replaces one with the same filename).
        if request.files.len() > 1 {
            let files = request
                .files
                .iter()
                .map(|path| {
                    if let Some(existing) =
                        find_matching_installed_file(target, request.fallback_folder, path, data, cx)
                    {
                        DroppedFile {
                            path: path.clone().into(),
                            source: existing.source,
                            fallback_folder: Some(existing.folder),
                            replace_old: Some(existing.path),
                        }
                    } else {
                        DroppedFile {
                            path: path.clone().into(),
                            source: ContentSource::Manual,
                            fallback_folder: request.fallback_folder,
                            replace_old: None,
                        }
                    }
                })
                .collect();
            install(files, DropTarget::Instance(target), &data.backend_handle, window, cx);
            return;
        }
    }

    window.close_all_dialogs(cx);
    let dialog = cx.new(|cx| DropInstallDialog::new(request, data, window, cx));
    window.open_dialog(cx, move |modal, window, cx| {
        dialog.update(cx, |dialog, cx| dialog.render(modal, window, cx))
    });
}

fn install(
    files: Arc<[DroppedFile]>,
    target: DropTarget,
    backend_handle: &BackendHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let modal_action = ModalAction::default();
    backend_handle.send(MessageToBackend::InstallDroppedFiles {
        files,
        target,
        modal_action: modal_action.clone(),
    });
    crate::modals::generic::show_notification(window, cx, t::instance::content::install::error().into(), modal_action);
}

/// Where the dropped files go, as picked in the dialog.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetChoice {
    Existing(InstanceID),
    NewInstance,
    NewServer,
}

enum Inspection {
    Loading,
    Loaded(DroppedFileInfo),
    Failed(SharedString),
}

struct DropInstallDialog {
    data: DataEntities,
    request: DropRequest,
    inspection: Option<Inspection>,
    picker: Entity<ProjectPicker>,
    target_select: Option<Entity<SelectState<NamedDropdown<TargetChoice>>>>,
    name_input: Entity<InputState>,
    /// The name last filled in for a new instance or server, which is replaced as more becomes
    /// known about the file until the user types their own.
    suggested_name: Option<SharedString>,
    version_select: Option<Entity<SelectState<SearchableVec<SharedString>>>>,
    loader: Loader,
    platform: ServerPlatform,
    eula_accepted: bool,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DropInstallDialog {
    fn new(request: DropRequest, data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let picker = cx.new(|cx| ProjectPicker::new(data.backend_handle.clone(), false, window, cx));
        let name_input = cx.new(|cx| InputState::new(window, cx));
        let mut subscriptions = vec![cx.observe_in(&picker, window, |this, _, window, cx| {
            this.suggest_name(window, cx);
            cx.notify();
        })];

        // The versions a new instance or server can be made on
        let versions = FrontendMetadata::request(&data.metadata, MetadataRequest::MinecraftVersionManifest, cx);
        subscriptions.push(cx.observe_in(&versions, window, |this, versions, window, cx| {
            this.fill_versions(&versions, window, cx);
        }));

        let target_select = request.target.is_none().then(|| {
            let mut entries: Vec<_> =
                data.instances.read(cx).entries.values().map(|entry| entry.read(cx).clone()).collect();
            entries.sort_by(|a, b| {
                (a.configuration.server.is_some(), a.title.to_lowercase())
                    .cmp(&(b.configuration.server.is_some(), b.title.to_lowercase()))
            });
            let mut items: Vec<_> = entries
                .iter()
                .map(|entry| NamedDropdownItem {
                    name: DropdownName::new(if entry.configuration.server.is_some() {
                        SharedString::from(t::instance::content::drop::server_entry(&entry.title))
                    } else {
                        entry.title.clone()
                    }),
                    item: TargetChoice::Existing(entry.id),
                })
                .collect();
            items.push(NamedDropdownItem {
                name: DropdownName::translated(t::instance::content::drop::new_instance),
                item: TargetChoice::NewInstance,
            });
            items.push(NamedDropdownItem {
                name: DropdownName::translated(t::instance::content::drop::new_server),
                item: TargetChoice::NewServer,
            });
            cx.new(|cx| {
                let mut select = SelectState::new(NamedDropdown::new(items), None, window, cx);
                select.set_selected_index(Some(IndexPath::default()), window, cx);
                select
            })
        });
        if let Some(select) = &target_select {
            subscriptions.push(cx.observe(select, |_, _, cx| cx.notify()));
        }

        let mut dialog = Self {
            data: data.clone(),
            inspection: None,
            picker,
            target_select,
            name_input,
            suggested_name: None,
            version_select: None,
            loader: Loader::Vanilla,
            platform: ServerPlatform::Vanilla,
            eula_accepted: false,
            _tasks: Vec::new(),
            _subscriptions: subscriptions,
            request,
        };
        dialog.fill_versions(&versions, window, cx);
        if dialog.request.files.len() == 1 {
            dialog.inspect(window, cx);
        }
        dialog
    }

    fn inspect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.inspection = Some(Inspection::Loading);
        let (send, recv) = tokio::sync::oneshot::channel();
        self.data.backend_handle.send(MessageToBackend::InspectDroppedFile {
            path: self.request.files[0].clone().into(),
            channel: send,
        });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = recv.await.unwrap_or_else(|_| Err("The launcher stopped answering".into()));
            _ = this.update_in(cx, |this, window, cx| {
                this.inspection = Some(match result {
                    Ok(info) => {
                        this.loader = suggested_loader(&info);
                        this.platform = suggested_platform(&info, this.loader);
                        if info.project.is_none() {
                            this.picker.update(cx, |picker, cx| picker.focus(window, cx));
                        }
                        Inspection::Loaded(info)
                    },
                    Err(error) => Inspection::Failed(SharedString::from(error.to_string())),
                });
                this.suggest_name(window, cx);
                this.select_suggested_version(window, cx);
                cx.notify();
            });
        }));
    }

    fn info(&self) -> Option<&DroppedFileInfo> {
        match &self.inspection {
            Some(Inspection::Loaded(info)) => Some(info),
            _ => None,
        }
    }

    /// The project the file is linked to: the one picked by hand, else the one found by its hash.
    fn project(&self, cx: &App) -> Option<ResolvedProject> {
        self.picker
            .read(cx)
            .resolved
            .clone()
            .or_else(|| self.info().and_then(|info| info.project.clone()))
    }

    /// A new instance or server is named after what's going into it, until a name is typed.
    fn suggest_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.name_input.read(cx).value();
        let typed_own = !current.is_empty() && self.suggested_name.as_ref() != Some(&current);
        if typed_own {
            return;
        }
        let name: Option<SharedString> = match (self.project(cx), self.info()) {
            (Some(project), _) => Some(SharedString::new(project.title)),
            (None, Some(info)) => Some(match &info.summary.name {
                Some(name) => SharedString::new(name.clone()),
                None => file_stem(&info.filename).into(),
            }),
            (None, None) => None,
        };
        if let Some(name) = name {
            self.suggested_name = Some(name.clone());
            self.name_input.update(cx, |input, cx| input.set_value(name, window, cx));
        }
    }

    fn fill_versions(
        &mut self,
        versions: &Entity<crate::entity::metadata::FrontendMetadataState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.version_select.is_some() {
            return;
        }
        let result: FrontendMetadataResult<MinecraftVersionManifest> = versions.read(cx).result();
        let FrontendMetadataResult::Loaded(manifest) = result else {
            return;
        };
        let releases: Vec<SharedString> = manifest
            .versions
            .iter()
            .filter(|version| matches!(version.r#type, MinecraftVersionType::Release))
            .map(|version| SharedString::new(version.id.as_str()))
            .collect();
        self.version_select =
            Some(cx.new(|cx| SelectState::new(SearchableVec::new(releases), None, window, cx).searchable(true)));
        self.select_suggested_version(window, cx);
    }

    /// The newest release the file was made for, or the newest release there is.
    fn select_suggested_version(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(select) = self.version_select.clone() else {
            return;
        };
        let suggested: Vec<SharedString> = self
            .info()
            .map(|info| {
                info.game_versions
                    .iter()
                    .rev()
                    .map(|version| SharedString::new(version.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        select.update(cx, |select, cx| {
            for version in &suggested {
                select.set_selected_value(version, window, cx);
                if select.selected_index(cx).is_some() {
                    return;
                }
            }
            select.set_selected_index(Some(IndexPath::default()), window, cx);
        });
    }

    fn target(&self, cx: &App) -> Option<TargetChoice> {
        match self.request.target {
            Some(id) => Some(TargetChoice::Existing(id)),
            None => self.target_select.as_ref()?.read(cx).selected_value().copied(),
        }
    }

    fn drop_target(&self, cx: &App) -> Option<DropTarget> {
        let name = || -> Option<Arc<str>> {
            let name = self.name_input.read(cx).value();
            let name = name.trim();
            (!name.is_empty()).then(|| Arc::from(name))
        };
        let version = || {
            self.version_select
                .as_ref()
                .and_then(|select| select.read(cx).selected_value().cloned())
                .map(|version| ustr::Ustr::from(version.as_str()))
        };
        Some(match self.target(cx)? {
            TargetChoice::Existing(id) => DropTarget::Instance(id),
            TargetChoice::NewInstance => DropTarget::NewInstance {
                name: name()?,
                minecraft_version: version()?,
                loader: self.loader,
            },
            TargetChoice::NewServer => DropTarget::NewServer {
                name: name()?,
                minecraft_version: version()?,
                platform: self.platform,
                eula_accepted: self.eula_accepted,
            },
        })
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.drop_target(cx) else {
            return;
        };
        let source = self.project(cx).map(|project| project.source).unwrap_or(ContentSource::Manual);
        let inspected_folder = self.info().and_then(|info| info.content_folder);
        let files: Arc<[DroppedFile]> = self
            .request
            .files
            .iter()
            .map(|path| DroppedFile {
                path: path.clone().into(),
                // Only a single file is identified, so only it can carry a project
                source: if self.request.files.len() == 1 {
                    source.clone()
                } else {
                    ContentSource::Manual
                },
                fallback_folder: if self.request.files.len() == 1 {
                    inspected_folder.or(self.request.fallback_folder)
                } else {
                    self.request.fallback_folder
                },
                replace_old: None,
            })
            .collect();
        window.close_dialog(cx);
        install(files, target, &self.data.backend_handle, window, cx);
    }

    fn render(&mut self, modal: Dialog, _window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        let title = match self.request.target.and_then(|id| self.data.instances.read(cx).entries.get(&id).cloned()) {
            Some(entry) => SharedString::from(t::instance::content::drop::into(&entry.read(cx).title)),
            None => t::instance::content::drop::title().into(),
        };
        let identified = self.info().is_some_and(|info| info.project.is_some());
        let unidentified = matches!(self.inspection, Some(Inspection::Loaded(_))) && !identified;
        let can_install = self.drop_target(cx).is_some()
            && !matches!(self.inspection, Some(Inspection::Loading) | Some(Inspection::Failed(_)));

        let content = v_flex()
            .gap_3()
            .child(self.render_files(cx))
            .when(unidentified, |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(div().text_sm().child(t::instance::content::drop::which_project()))
                        .child(self.picker.clone()),
                )
            })
            .when(self.request.target.is_none(), |this| this.child(self.render_target(cx)));

        modal.title(title).width(px(520.0)).on_ok(|_, _, _| false).child(content).footer(
            h_flex()
                .gap_2()
                .w_full()
                .child(
                    Button::new("cancel")
                        .flex_1()
                        .label(t::common::cancel())
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                )
                .child(
                    Button::new("install")
                        .flex_1()
                        .success()
                        .label(t::instance::content::install::label())
                        .disabled(!can_install)
                        .on_click(cx.listener(|this, _, window, cx| this.install(window, cx))),
                ),
        )
    }

    /// The file, as it'll show once installed: icon, name and version. Several files are listed.
    fn render_files(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.request.files.len() > 1 {
            let names = self.request.files.iter().take(6).map(|path| {
                div().text_sm().truncate().child(SharedString::from(
                    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
                ))
            });
            return v_flex()
                .gap_1()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(t::instance::content::drop::files(self.request.files.len())),
                )
                .children(names)
                .when(self.request.files.len() > 6, |this| {
                    this.child(div().text_sm().text_color(theme.muted_foreground).child("…"))
                })
                .into_any_element();
        }

        let info = match &self.inspection {
            None | Some(Inspection::Loading) => {
                let filename = self.request.files[0].file_name().map(|name| name.to_string_lossy().into_owned());
                return h_flex()
                    .gap_2()
                    .child(Spinner::new())
                    .child(t::instance::content::drop::reading(&filename.unwrap_or_default()))
                    .into_any_element();
            },
            Some(Inspection::Failed(error)) => {
                return div().text_color(theme.danger).child(error.clone()).into_any_element();
            },
            Some(Inspection::Loaded(info)) => info,
        };
        let project = self.project(cx);

        let icon = match (&project, &info.summary.png_icon) {
            (
                Some(ResolvedProject {
                    icon_url: Some(url), ..
                }),
                _,
            ) => gpui::img(SharedUri::from(url.to_string()))
                .size_12()
                .rounded(theme.radius)
                .into_any_element(),
            (_, Some(icon)) => png_render_cache::render(icon.clone(), cx)
                .size_12()
                .rounded(theme.radius)
                .into_any_element(),
            _ => div()
                .size_12()
                .rounded(theme.radius)
                .bg(theme.secondary)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    gpui_component::Icon::new(PandoraIcon::FileQuestionMark)
                        .size_6()
                        .text_color(theme.muted_foreground),
                )
                .into_any_element(),
        };
        let name: SharedString = match (&project, &info.summary.name) {
            (Some(project), _) => SharedString::new(project.title.clone()),
            (None, Some(name)) => SharedString::new(name.clone()),
            (None, None) => file_stem(&info.filename).into(),
        };
        let version: Option<SharedString> = info.version_name.clone().map(SharedString::new).or_else(|| {
            (!info.summary.version_str.is_empty()).then(|| SharedString::new(info.summary.version_str.clone()))
        });
        let found_on: SharedString = match project.as_ref().map(|project| &project.source) {
            Some(ContentSource::ModrinthProject { .. }) => t::modrinth::name().into(),
            Some(ContentSource::CurseforgeProject { .. }) => t::curseforge::name().into(),
            _ => t::instance::content::drop::unknown_project().into(),
        };

        h_flex()
            .gap_3()
            .child(icon)
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(h_flex().gap_2().items_baseline().child(div().text_lg().truncate().child(name)).when_some(
                        version,
                        |this, version| {
                            this.child(div().text_sm().text_color(theme.muted_foreground).truncate().child(version))
                        },
                    ))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(SharedString::from(format!("{} · {found_on}", info.filename))),
                    ),
            )
            .into_any_element()
    }

    fn render_target(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(select) = &self.target_select else {
            return gpui::Empty.into_any_element();
        };
        let target = self.target(cx);
        let mut section = v_flex()
            .gap_2()
            .child(div().text_sm().child(t::instance::content::drop::install_into()))
            .child(Select::new(select).w_full());

        if matches!(target, Some(TargetChoice::NewInstance) | Some(TargetChoice::NewServer)) {
            section =
                section
                    .child(Input::new(&self.name_input))
                    .when_some(self.version_select.as_ref(), |this, select| {
                        this.child(
                            Select::new(select)
                                .w_full()
                                .title_prefix(format!("{}: ", t::instance::game_version()))
                                .search_placeholder(t::common::search()),
                        )
                    });
        }

        match target {
            Some(TargetChoice::NewInstance) => {
                const LOADERS: [Loader; 4] = [Loader::Vanilla, Loader::Fabric, Loader::Forge, Loader::NeoForge];
                section = section.child(LOADERS.iter().enumerate().fold(
                    ButtonGroup::new("drop-loader").outline().w_full().on_click(cx.listener(
                        |this, clicked: &Vec<usize>, _, cx| {
                            if let Some(loader) = clicked.first().and_then(|index| LOADERS.get(*index)) {
                                this.loader = *loader;
                                cx.notify();
                            }
                        },
                    )),
                    |group, (index, loader)| {
                        group.child(
                            Button::new(("drop-loader", index))
                                .flex_1()
                                .label(loader.pretty_name())
                                .selected(self.loader == *loader),
                        )
                    },
                ));
            },
            Some(TargetChoice::NewServer) => {
                let platforms: Vec<ServerPlatform> = ServerPlatform::iter().collect();
                section = section
                    .child(platforms.iter().enumerate().fold(
                        ButtonGroup::new("drop-platform").outline().w_full().on_click(cx.listener({
                            let platforms = platforms.clone();
                            move |this, clicked: &Vec<usize>, _, cx| {
                                if let Some(platform) = clicked.first().and_then(|index| platforms.get(*index)) {
                                    this.platform = *platform;
                                    cx.notify();
                                }
                            }
                        })),
                        |group, (index, platform)| {
                            group.child(
                                Button::new(("drop-platform", index))
                                    .flex_1()
                                    .small()
                                    .label(platform.pretty_name())
                                    .selected(self.platform == *platform),
                            )
                        },
                    ))
                    .child(
                        Checkbox::new("drop-eula")
                            .label(t::server::eula::agree())
                            .checked(self.eula_accepted)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.eula_accepted = *checked;
                                cx.notify();
                            })),
                    );
            },
            _ => {},
        }
        section.into_any_element()
    }
}

/// The loader a new instance for the file should have: the one it was published for, or the one
/// its own metadata names.
fn suggested_loader(info: &DroppedFileInfo) -> Loader {
    info.loaders
        .iter()
        .copied()
        .find(|loader| loader.is_client_loader())
        .or_else(|| info.summary.extra.mod_loader())
        .unwrap_or(Loader::Vanilla)
}

fn suggested_platform(info: &DroppedFileInfo, loader: Loader) -> ServerPlatform {
    match info.summary.extra {
        bridge::instance::ContentType::BukkitPlugin | bridge::instance::ContentType::PaperPlugin => {
            ServerPlatform::Paper
        },
        _ => match loader {
            Loader::Fabric => ServerPlatform::Fabric,
            Loader::Forge => ServerPlatform::Forge,
            Loader::NeoForge => ServerPlatform::NeoForge,
            _ => ServerPlatform::Vanilla,
        },
    }
}

fn file_stem(filename: &str) -> String {
    let name = filename.strip_suffix(".disabled").unwrap_or(filename);
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => name.to_string(),
    }
}

/// A searchable input for finding a Modrinth or CurseForge project by name, link, or ID, showing
/// live suggestions with icons and which site each result comes from.
pub struct ProjectPicker {
    backend_handle: BackendHandle,
    input: Entity<InputState>,
    input_bounds: Bounds<Pixels>,
    show_selected_preview: bool,
    pub resolved: Option<ResolvedProject>,
    suggestions: Vec<ResolvedProject>,
    dropdown_open: bool,
    searching: bool,
    error: Option<SharedString>,
    selected_title: Option<SharedString>,
    search_seq: u64,
    _task: Task<()>,
    _subscription: Subscription,
}

impl ProjectPicker {
    pub fn new(
        backend_handle: BackendHandle,
        show_selected_preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t::instance::content::drop::project_placeholder()));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => this.on_input_change(window, cx),
            InputEvent::PressEnter { .. } => this.on_press_enter(window, cx),
            InputEvent::Focus => {
                if this.resolved.is_none() && (!this.suggestions.is_empty() || this.searching) {
                    this.dropdown_open = true;
                    window.refresh();
                    cx.notify();
                }
            },
            _ => {},
        });
        Self {
            backend_handle,
            input,
            input_bounds: Bounds::default(),
            show_selected_preview,
            resolved: None,
            suggestions: Vec::new(),
            dropdown_open: false,
            searching: false,
            error: None,
            selected_title: None,
            search_seq: 0,
            _task: Task::ready(()),
            _subscription: subscription,
        }
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn on_input_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value();
        let trimmed = value.trim();
        if self.selected_title.as_deref() == Some(trimmed) {
            return;
        }
        self.selected_title = None;
        self.resolved = None;
        self.error = None;

        if trimmed.is_empty() {
            self.search_seq += 1;
            self.suggestions.clear();
            self.dropdown_open = false;
            self.searching = false;
            self._task = Task::ready(());
            window.refresh();
            cx.notify();
            return;
        }

        self.start_search(trimmed.to_string(), true, false, window, cx);
    }

    fn on_press_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dropdown_open && !self.searching && !self.suggestions.is_empty() {
            let first = self.suggestions[0].clone();
            self.select_project(first, window, cx);
            return;
        }
        let value = self.input.read(cx).value();
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            self.start_search(trimmed.to_string(), false, true, window, cx);
        }
    }

    fn start_search(
        &mut self,
        query: String,
        debounce: bool,
        auto_select_single: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_seq += 1;
        let seq = self.search_seq;
        self.dropdown_open = true;
        self.searching = true;
        self.error = None;
        window.refresh();
        cx.notify();

        let backend_handle = self.backend_handle.clone();
        self._task = cx.spawn_in(window, async move |this, cx| {
            if debounce {
                cx.background_executor().timer(Duration::from_millis(250)).await;
            }
            let (send, recv) = tokio::sync::oneshot::channel();
            backend_handle.send(MessageToBackend::SearchProjects {
                query: query.into(),
                channel: send,
            });
            let result = recv.await.unwrap_or_else(|_| Err("The launcher stopped answering".into()));
            _ = this.update_in(cx, |this, window, cx| {
                if this.search_seq != seq {
                    return;
                }
                this.searching = false;
                match result {
                    Ok(projects) => {
                        this.suggestions = projects.to_vec();
                        if auto_select_single && this.suggestions.len() == 1 {
                            let project = this.suggestions[0].clone();
                            this.select_project(project, window, cx);
                            return;
                        }
                    },
                    Err(error) => {
                        this.suggestions.clear();
                        this.error = Some(SharedString::from(error.to_string()));
                    },
                }
                window.refresh();
                cx.notify();
            });
        });
    }

    fn select_project(&mut self, project: ResolvedProject, window: &mut Window, cx: &mut Context<Self>) {
        let title = SharedString::from(project.title.trim().to_string());
        self.selected_title = Some(title.clone());
        self.resolved = Some(project);
        self.dropdown_open = false;
        self.searching = false;
        self.error = None;
        self.input.update(cx, |input, cx| input.set_value(title, window, cx));
        window.refresh();
        cx.notify();
    }
}

impl Render for ProjectPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let bounds = self.input_bounds;

        v_flex()
            .gap_1p5()
            .child(
                div()
                    .relative()
                    .w_full()
                    .on_prepaint({
                        let entity = cx.entity();
                        move |bounds, _, cx| entity.update(cx, |this, _| this.input_bounds = bounds)
                    })
                    .child(
                        h_flex()
                            .relative()
                            .w_full()
                            .child(div().flex_1().child(Input::new(&self.input).cleanable(true)))
                            .when(self.searching, |this| {
                                this.child(
                                    div().absolute().right_8().flex().items_center().child(Spinner::new().small()),
                                )
                            }),
                    )
                    .when(self.dropdown_open, |this| {
                        let mut list = v_flex().p_1().gap_0p5();
                        if self.searching && self.suggestions.is_empty() {
                            list = list.child(
                                h_flex()
                                    .gap_2()
                                    .px_2()
                                    .py_2()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(Spinner::new().small())
                                    .child(t::instance::content::drop::searching()),
                            );
                        } else if let Some(error) = self.error.clone() {
                            list = list.child(div().px_2().py_2().text_sm().text_color(theme.danger).child(error));
                        } else if self.suggestions.is_empty() {
                            list = list.child(
                                div()
                                    .px_2()
                                    .py_2()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(t::instance::content::drop::no_results()),
                            );
                        } else {
                            for (idx, project) in self.suggestions.iter().enumerate() {
                                let site = match project.source {
                                    ContentSource::CurseforgeProject { .. } => t::curseforge::name(),
                                    _ => t::modrinth::name(),
                                };
                                let icon = match &project.icon_url {
                                    Some(url) => gpui::img(SharedUri::from(url.to_string()))
                                        .size_6()
                                        .flex_shrink_0()
                                        .rounded(px(4.0))
                                        .into_any_element(),
                                    None => gpui_component::Icon::new(PandoraIcon::PackageOpen)
                                        .size_6()
                                        .flex_shrink_0()
                                        .text_color(theme.muted_foreground)
                                        .into_any_element(),
                                };
                                let project_for_click = project.clone();
                                list = list.child(
                                    h_flex()
                                        .id(("project-suggestion", idx))
                                        .w_full()
                                        .items_center()
                                        .gap_2()
                                        .px_2()
                                        .py_1p5()
                                        .rounded(theme.radius)
                                        .cursor_pointer()
                                        .hover(|style| style.bg(theme.list_hover))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.select_project(project_for_click.clone(), window, cx);
                                            }),
                                        )
                                        .child(icon)
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_sm()
                                                .child(SharedString::new(project.title.clone())),
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child(site),
                                        ),
                                );
                            }
                        }

                        this.child(
                            deferred(
                                anchored()
                                    .position(point(bounds.origin.x, bounds.bottom() + px(4.0)))
                                    .snap_to_window_with_margin(px(8.0))
                                    .child(
                                        div()
                                            .occlude()
                                            .when(bounds.size.width > px(0.0), |this| this.w(bounds.size.width))
                                            .max_h(px(260.0))
                                            .overflow_y_scrollbar()
                                            .popover_style(cx)
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                            .on_mouse_down_out(cx.listener(
                                                |this, event: &MouseDownEvent, window, cx| {
                                                    if !this.input_bounds.contains(&event.position) {
                                                        this.dropdown_open = false;
                                                        window.refresh();
                                                        cx.notify();
                                                    }
                                                },
                                            ))
                                            .child(list),
                                    ),
                            )
                            // Above Dialog's deferred layer (10 + layer), matching gpui_base::POPUP_PRIORITY
                            .with_priority(100),
                        )
                    }),
            )
            .when(!self.dropdown_open, |this| {
                this.when(self.show_selected_preview, |this| {
                    this.when_some(self.resolved.as_ref(), |this, project| {
                        let site = match project.source {
                            ContentSource::CurseforgeProject { .. } => t::curseforge::name(),
                            _ => t::modrinth::name(),
                        };
                        this.child(
                            h_flex()
                                .gap_2()
                                .p_2()
                                .rounded(theme.radius)
                                .border_1()
                                .border_color(theme.border)
                                .child(match &project.icon_url {
                                    Some(url) => gpui::img(SharedUri::from(url.to_string()))
                                        .size_8()
                                        .rounded(theme.radius)
                                        .into_any_element(),
                                    None => {
                                        gpui_component::Icon::new(PandoraIcon::PackageOpen).size_8().into_any_element()
                                    },
                                })
                                .child(
                                    div().flex_1().min_w_0().truncate().child(SharedString::new(project.title.clone())),
                                )
                                .child(div().flex_shrink_0().text_sm().text_color(theme.muted_foreground).child(site)),
                        )
                    })
                })
                .when_some(self.error.clone(), |this, error| {
                    this.child(div().text_sm().text_color(theme.danger).child(error))
                })
            })
    }
}

/// Links content added by hand to the project it's a build of, so it gets that project's icon and
/// update checks.
pub fn open_set_project(
    instance: InstanceID,
    content_folder: ContentFolder,
    hashes: Arc<[[u8; 20]]>,
    name: SharedString,
    backend_handle: BackendHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let picker = cx.new(|cx| ProjectPicker::new(backend_handle.clone(), true, window, cx));
    let focus_picker = picker.clone();
    window.open_dialog(cx, move |modal, _, cx| {
        let resolved = picker.read(cx).resolved.clone();
        modal
            .title(t::instance::content::set_project::title(&name))
            .width(px(480.0))
            .on_ok(|_, _, _| false)
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t::instance::content::set_project::description()),
                    )
                    .child(picker.clone()),
            )
            .footer(
                h_flex()
                    .gap_2()
                    .w_full()
                    .child(
                        Button::new("cancel")
                            .flex_1()
                            .label(t::common::cancel())
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("save")
                            .flex_1()
                            .success()
                            .label(t::common::save())
                            .disabled(resolved.is_none())
                            .on_click({
                                let backend_handle = backend_handle.clone();
                                let hashes = hashes.clone();
                                move |_, window, cx| {
                                    let Some(project) = resolved.clone() else {
                                        return;
                                    };
                                    backend_handle.send(MessageToBackend::SetContentProject {
                                        id: instance,
                                        content_folder,
                                        hashes: hashes.clone(),
                                        source: project.source,
                                    });
                                    window.close_dialog(cx);
                                }
                            }),
                    ),
            )
    });
    focus_picker.update(cx, |picker, cx| picker.focus(window, cx));
}
