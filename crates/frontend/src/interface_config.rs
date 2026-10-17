use std::{cmp::Ordering, io::Write, path::Path, sync::Arc, time::Duration};

use bridge::{
    handle::BackendHandle,
    instance::{ContentFolder, InstanceContentSummary, InstanceID},
    message::MessageToBackend,
};
use gpui::{App, BorrowAppContext, SharedString, Task};
use rand::RngCore;
use rustc_hash::FxHashSet;
use schema::{
    content_label::HexColor, curseforge::CurseforgeClassId, modrinth::ModrinthProjectType, quickplay::QuickplayPreset,
};
use serde::{Deserialize, Serialize};

use crate::{
    component::{
        background_effect::{BackgroundEffect, EffectQuality},
        named_dropdown::DropdownName,
    },
    pages::instance::instance_page::InstanceSubpageType,
    ui::PageType,
};

struct InterfaceConfigHolder {
    config: InterfaceConfig,
    write_task: Option<Task<()>>,
    path: Arc<Path>,
}

impl gpui::Global for InterfaceConfigHolder {}

#[derive(Debug, Serialize, Deserialize)]
pub struct InterfaceConfig {
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub language: t::Language,

    // Theme
    #[serde(
        default,
        skip_serializing_if = "schema::skip_if_none",
        deserialize_with = "schema::try_deserialize"
    )]
    pub active_theme: Option<SharedString>,
    #[serde(
        default,
        skip_serializing_if = "schema::skip_if_none",
        deserialize_with = "schema::try_deserialize"
    )]
    pub font_family: Option<SharedString>,
    #[serde(
        default,
        skip_serializing_if = "schema::skip_if_none",
        deserialize_with = "schema::try_deserialize"
    )]
    pub font_size: Option<i32>,

    // Window state
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub main_window_bounds: WindowBounds,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub game_output_window_bounds: WindowBounds,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub main_page: PageType,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub page_path: Arc<[PageType]>,

    // Sidebar options
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub sidebar_width: f32,
    #[serde(default = "schema::default_true", deserialize_with = "schema::try_deserialize")]
    pub show_sidebar_icons: bool,
    #[serde(default = "schema::default_true", deserialize_with = "schema::try_deserialize")]
    pub show_quickplay_page: bool,

    // Instance management
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub quick_delete_mods: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub quick_delete_instance: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub quick_delete_skins: bool,
    pub preferred_add_content_source: PreferredAddContentSource,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_mods_sort_key: InstanceContentSortKey,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_mods_sort_enabled_first: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_plugins_sort_key: InstanceContentSortKey,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_plugins_sort_enabled_first: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_resourcepacks_sort_key: InstanceContentSortKey,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_resourcepacks_sort_enabled_first: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_shaders_sort_key: InstanceContentSortKey,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_shaders_sort_enabled_first: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_mods_display: ContentListDisplay,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_plugins_display: ContentListDisplay,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_resourcepacks_display: ContentListDisplay,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_shaders_display: ContentListDisplay,
    /// The categories offered when labelling content, shared by every instance and server.
    /// Which category an item is in is kept with the item itself, in its auxiliary file.
    #[serde(
        default = "default_content_categories",
        deserialize_with = "deserialize_content_categories"
    )]
    pub content_categories: Vec<ContentCategory>,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub show_snapshots_in_create_instance: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instances_view_mode: ViewMode,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub servers_view_mode: ViewMode,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_group_order: Vec<Arc<str>>,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_groups_closed: FxHashSet<SharedString>,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub instance_subpage: InstanceSubpageType,

    // Content
    #[serde(default = "schema::default_true", deserialize_with = "schema::try_deserialize")]
    pub content_install_latest: bool,
    /// Whether browsing content for an instance starts out narrowed to the instance's own Minecraft
    /// version. Replaces `content_filter_version`, which was off by default.
    #[serde(default = "schema::default_true", deserialize_with = "schema::try_deserialize")]
    pub content_filter_instance_version: bool,
    #[serde(
        default = "default_modrinth_project_type",
        deserialize_with = "schema::try_deserialize"
    )]
    pub modrinth_page_project_type: ModrinthProjectType,
    #[serde(
        default = "default_curseforge_class_id",
        deserialize_with = "schema::try_deserialize"
    )]
    pub curseforge_page_class_id: CurseforgeClassId,

    // Window options
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub hide_main_window_on_launch: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub live_game_output_display: LiveGameOutputDisplay,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub reuse_game_output_window: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub quit_on_main_closed: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub use_os_titlebar: bool,

    // Privacy options
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub hide_usernames: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub hide_skins: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub hide_server_addresses: bool,

    // Skins page
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub collapse_capes_in_skins_page: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub skin_list_sort_desc: bool,
    #[serde(default = "schema::default_true", deserialize_with = "schema::try_deserialize")]
    pub skin_list_show_3d: bool,
    #[serde(default = "default_zoom", deserialize_with = "schema::try_deserialize")]
    pub player_model_zoom: i32,
    #[serde(default = "schema::default_true", deserialize_with = "schema::try_deserialize")]
    pub player_model_animate: bool,

    // Quickplay
    /// The preset the old quickplay page last had picked. Only read to tell which preset the old
    /// hidden quickplay instance holds when it is carried over into the preset instances.
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub quickplay_preset: QuickplayPreset,
    /// Whether the Vanilla, Performance and Expanded instances have been set up. They are only
    /// ever set up once, so deleting them sticks.
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub created_preset_instances: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub background_effect: BackgroundEffect,
    #[serde(default = "schema::default_true", deserialize_with = "schema::try_deserialize")]
    pub background_animated: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub background_quality: EffectQuality,
    /// An image shown behind the quickplay page in place of the effect's own backdrop.
    #[serde(
        default,
        skip_serializing_if = "schema::skip_if_none",
        deserialize_with = "schema::try_deserialize"
    )]
    pub custom_background: Option<Arc<Path>>,
    #[serde(default = "schema::default_true", deserialize_with = "schema::try_deserialize")]
    pub skin_animation: bool,
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub snow_effect: bool,

    // Groups as they were stored before upstream moved group membership onto the instance itself.
    // Kept only so `migrate_instance_groups` can carry them over, and cleared once it has.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "schema::try_deserialize"
    )]
    pub instance_groups: Vec<LegacyInstanceGroup>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "schema::try_deserialize"
    )]
    pub instance_group_assignments: Vec<(InstanceID, u64)>,
    #[serde(
        default,
        skip_serializing_if = "std::ops::Not::not",
        deserialize_with = "schema::try_deserialize"
    )]
    pub migrated_instance_group_order: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LegacyInstanceGroup {
    pub id: u64,
    pub name: String,
    pub collapsed: bool,
}

pub const DEFAULT_THEME: &'static str = "Default Dark";

#[cfg(windows)]
pub const DEFAULT_FONT: &'static str = "Inter 24pt 24pt";
#[cfg(not(windows))]
pub const DEFAULT_FONT: &'static str = "Inter 24pt";

pub const DEFAULT_FONT_SIZE: i32 = 16;

impl InterfaceConfig {
    pub fn content_list_display(&self, folder: ContentFolder) -> ContentListDisplay {
        match folder {
            ContentFolder::Mods => self.instance_mods_display,
            ContentFolder::ResourcePacks => self.instance_resourcepacks_display,
            ContentFolder::Shaders => self.instance_shaders_display,
            ContentFolder::Plugins => self.instance_plugins_display,
        }
    }

    pub fn content_list_display_mut(&mut self, folder: ContentFolder) -> &mut ContentListDisplay {
        match folder {
            ContentFolder::Mods => &mut self.instance_mods_display,
            ContentFolder::ResourcePacks => &mut self.instance_resourcepacks_display,
            ContentFolder::Shaders => &mut self.instance_shaders_display,
            ContentFolder::Plugins => &mut self.instance_plugins_display,
        }
    }

    pub fn set_active_theme(&mut self, theme: SharedString) {
        if theme == DEFAULT_THEME {
            self.active_theme = None;
        } else {
            self.active_theme = Some(theme);
        }
    }

    pub fn set_font_family(&mut self, font_family: SharedString) {
        if font_family == DEFAULT_FONT {
            self.font_family = None;
        } else {
            self.font_family = Some(font_family);
        }
    }

    pub fn set_font_size(&mut self, font_size: i32) {
        if font_size == DEFAULT_FONT_SIZE {
            self.font_size = None;
        } else {
            self.font_size = Some(font_size);
        }
    }

    pub fn apply_theme(cx: &mut App, log: bool) {
        cx.update_global::<InterfaceConfigHolder, _>(|holder, cx| {
            let registry = gpui_component::ThemeRegistry::global(cx);
            let theme_config = if let Some(active_theme) = &holder.config.active_theme {
                if let Some(theme_config) = registry.themes().get(active_theme).cloned() {
                    theme_config
                } else {
                    if log {
                        log::warn!("Unable to find theme with name {}, using default theme", active_theme);
                    }
                    registry.default_dark_theme().clone()
                }
            } else {
                registry.default_dark_theme().clone()
            };

            let theme = gpui_component::Theme::global_mut(cx);

            theme.apply_config(&theme_config);

            theme.font_family = holder.config.font_family.clone().unwrap_or(SharedString::new_static(DEFAULT_FONT));
            theme.font_size = gpui::px(holder.config.font_size.unwrap_or(DEFAULT_FONT_SIZE).clamp(8, 32) as f32);
            theme.scrollbar_mode = gpui_component::scroll::ScrollbarMode::Always;
        });
        cx.refresh_windows();
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, strum::EnumIter)]
#[serde(rename_all = "lowercase")]
pub enum LiveGameOutputDisplay {
    #[default]
    TabOnInstancePage,
    SeparateWindow,
    Hidden,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, strum::EnumIter)]
#[serde(rename_all = "lowercase")]
pub enum InstanceContentSortKey {
    #[default]
    Name,
    ModId,
    Filename,
    ModifiedTime,
    FileSize,
    /// Grouped by the category the user filed each item under.
    Category,
}

impl InstanceContentSortKey {
    pub fn name(self) -> DropdownName {
        match self {
            InstanceContentSortKey::Name => DropdownName::translated(t::instance::content::sort_key::name),
            InstanceContentSortKey::ModId => DropdownName::translated(t::instance::content::sort_key::mod_id),
            InstanceContentSortKey::Filename => DropdownName::translated(t::instance::content::sort_key::filename),
            InstanceContentSortKey::ModifiedTime => {
                DropdownName::translated(t::instance::content::sort_key::modified_time)
            },
            InstanceContentSortKey::FileSize => DropdownName::translated(t::instance::content::sort_key::filesize),
            InstanceContentSortKey::Category => DropdownName::translated(t::instance::content::sort_key::category),
        }
    }

    pub fn compare(self, a: &InstanceContentSummary, b: &InstanceContentSummary) -> Ordering {
        match self {
            InstanceContentSortKey::Name => {
                let name_a = a
                    .content_summary
                    .name
                    .as_deref()
                    .or(a.content_summary.id.as_deref())
                    .unwrap_or(&*a.filename);
                let name_b = b
                    .content_summary
                    .name
                    .as_deref()
                    .or(b.content_summary.id.as_deref())
                    .unwrap_or(&*b.filename);
                lexical_sort::natural_lexical_cmp(name_a, name_b)
            },
            InstanceContentSortKey::ModId => {
                let name_a = a
                    .content_summary
                    .id
                    .as_deref()
                    .or(a.content_summary.name.as_deref())
                    .unwrap_or(&*a.filename);
                let name_b = b
                    .content_summary
                    .id
                    .as_deref()
                    .or(b.content_summary.name.as_deref())
                    .unwrap_or(&*b.filename);
                lexical_sort::natural_lexical_cmp(name_a, name_b)
            },
            InstanceContentSortKey::Filename => {
                let name_a = &*a.filename;
                let name_b = &*b.filename;
                lexical_sort::natural_lexical_cmp(name_a, name_b)
            },
            InstanceContentSortKey::ModifiedTime => a.modified_unix_ms.cmp(&b.modified_unix_ms).reverse(),
            InstanceContentSortKey::FileSize => a
                .content_summary
                .filesize
                .unwrap_or(0)
                .cmp(&b.content_summary.filesize.unwrap_or(0))
                .reverse(),
            InstanceContentSortKey::Category => crate::component::content_label::compare_categories(
                a.label.category.as_deref(),
                b.label.category.as_deref(),
            ),
        }
    }
}

/// How rows are laid out in a mod, resource pack, shader or plugin list.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, strum::EnumIter)]
#[serde(rename_all = "lowercase")]
pub enum ContentListViewMode {
    /// Icon, name, version, authors and filename.
    #[default]
    Standard,
    /// One line per item: the toggle, the filename and the item's buttons.
    Compact,
}

impl ContentListViewMode {
    pub fn name(self) -> &'static str {
        match self {
            ContentListViewMode::Standard => t::instance::content::view::standard(),
            ContentListViewMode::Compact => t::instance::content::view::compact(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ContentListDisplay {
    #[serde(default, deserialize_with = "schema::try_deserialize")]
    pub view_mode: ContentListViewMode,
    #[serde(default = "schema::default_true", deserialize_with = "schema::try_deserialize")]
    pub show_icons: bool,
}

impl Default for ContentListDisplay {
    fn default() -> Self {
        Self {
            view_mode: ContentListViewMode::Standard,
            show_icons: true,
        }
    }
}

impl ContentListDisplay {
    pub fn is_compact(self) -> bool {
        self.view_mode == ContentListViewMode::Compact
    }

    /// Compact rows never have icons, whatever `show_icons` says.
    pub fn icons_visible(self) -> bool {
        !self.is_compact() && self.show_icons
    }
}

/// A category content can be put in.
///
/// The built-in ones are Modrinth categories, so they take their translated name and icon from
/// there; any other id is a name the user made up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentCategory {
    pub id: Arc<str>,
    #[serde(
        default,
        skip_serializing_if = "schema::skip_if_none",
        deserialize_with = "schema::try_deserialize"
    )]
    pub color: Option<HexColor>,
}

/// Not a Modrinth category, but given a translated name all the same.
pub const OTHER_CONTENT_CATEGORY: &str = "other";

pub fn default_content_categories() -> Vec<ContentCategory> {
    [
        ("library", Some(0x64748b)),
        ("worldgen", Some(0x22c55e)),
        ("combat", Some(0xef4444)),
        ("optimization", Some(0xeab308)),
        ("utility", Some(0x3b82f6)),
        ("game-mechanics", Some(0xa855f7)),
        (OTHER_CONTENT_CATEGORY, None),
    ]
    .into_iter()
    .map(|(id, color)| ContentCategory {
        id: id.into(),
        color: color.map(HexColor::new),
    })
    .collect()
}

/// Drops entries that don't parse instead of every category, and only falls back to the defaults
/// when there's no list at all, so a user who deleted every category keeps it that way.
fn deserialize_content_categories<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ContentCategory>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let Some(entries) = value.as_array() else {
        return Ok(default_content_categories());
    };
    Ok(entries.iter().filter_map(|entry| ContentCategory::deserialize(entry).ok()).collect())
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PreferredAddContentSource {
    #[default]
    Modrinth,
    CurseForge,
    File,
}

fn default_zoom() -> i32 {
    100
}

fn default_modrinth_project_type() -> ModrinthProjectType {
    ModrinthProjectType::Mod
}

fn default_curseforge_class_id() -> CurseforgeClassId {
    CurseforgeClassId::Mod
}

impl Default for InterfaceConfig {
    fn default() -> Self {
        Self {
            language: Default::default(),
            active_theme: Default::default(),
            font_family: None,
            font_size: None,
            main_window_bounds: Default::default(),
            game_output_window_bounds: Default::default(),
            main_page: Default::default(),
            page_path: Default::default(),
            sidebar_width: Default::default(),
            show_sidebar_icons: true,
            show_quickplay_page: true,
            quick_delete_mods: Default::default(),
            quick_delete_instance: Default::default(),
            quick_delete_skins: Default::default(),
            preferred_add_content_source: Default::default(),
            instance_mods_sort_key: Default::default(),
            instance_mods_sort_enabled_first: Default::default(),
            instance_plugins_sort_key: Default::default(),
            instance_plugins_sort_enabled_first: Default::default(),
            instance_resourcepacks_sort_key: Default::default(),
            instance_resourcepacks_sort_enabled_first: Default::default(),
            instance_shaders_sort_key: Default::default(),
            instance_shaders_sort_enabled_first: Default::default(),
            instance_mods_display: Default::default(),
            instance_plugins_display: Default::default(),
            instance_resourcepacks_display: Default::default(),
            instance_shaders_display: Default::default(),
            content_categories: default_content_categories(),
            content_install_latest: true,
            content_filter_instance_version: true,
            modrinth_page_project_type: default_modrinth_project_type(),
            curseforge_page_class_id: default_curseforge_class_id(),
            hide_main_window_on_launch: false,
            live_game_output_display: LiveGameOutputDisplay::default(),
            reuse_game_output_window: false,
            quit_on_main_closed: false,
            use_os_titlebar: false,
            hide_server_addresses: false,
            hide_usernames: false,
            hide_skins: false,
            show_snapshots_in_create_instance: Default::default(),
            instances_view_mode: Default::default(),
            servers_view_mode: Default::default(),
            instance_group_order: Vec::new(),
            instance_groups_closed: Default::default(),
            instance_subpage: Default::default(),
            collapse_capes_in_skins_page: false,
            skin_list_sort_desc: false,
            skin_list_show_3d: true,
            instance_groups: Default::default(),
            migrated_instance_group_order: false,
            instance_group_assignments: Default::default(),
            player_model_zoom: default_zoom(),
            player_model_animate: true,
            quickplay_preset: QuickplayPreset::Vanilla,
            created_preset_instances: false,
            background_effect: BackgroundEffect::default(),
            background_animated: true,
            background_quality: EffectQuality::default(),
            custom_background: None,
            skin_animation: true,
            snow_effect: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WindowBounds {
    #[default]
    Inherit,
    Windowed {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    Maximized {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    Fullscreen {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, strum::EnumIter)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    #[default]
    Cards,
    List,
}

impl ViewMode {
    pub fn name(self) -> DropdownName {
        match self {
            ViewMode::Cards => DropdownName::translated(t::common::layout::cards),
            ViewMode::List => DropdownName::translated(t::common::layout::list),
        }
    }
}

impl InterfaceConfig {
    pub fn init(cx: &mut App, path: Arc<Path>) {
        cx.set_global(InterfaceConfigHolder {
            config: try_read_json(&path),
            write_task: None,
            path,
        });
    }

    pub fn get(cx: &App) -> &Self {
        &cx.global::<InterfaceConfigHolder>().config
    }

    pub fn force_save(cx: &mut App) {
        cx.global_mut::<InterfaceConfigHolder>().write_to_disk();
    }

    pub fn get_mut(cx: &mut App) -> &mut Self {
        if cx.global::<InterfaceConfigHolder>().write_task.is_none() {
            let task = cx.spawn(async |app| {
                app.background_executor().timer(Duration::from_secs(5)).await;
                _ = app.update_global::<InterfaceConfigHolder, _>(|holder, _| {
                    holder.write_to_disk();
                });
            });

            let holder = cx.global_mut::<InterfaceConfigHolder>();
            holder.write_task = Some(task);
            &mut holder.config
        } else {
            &mut cx.global_mut::<InterfaceConfigHolder>().config
        }
    }
}

impl InterfaceConfigHolder {
    fn write_to_disk(&mut self) {
        self.write_task = None;
        let Ok(bytes) = serde_json::to_vec(&self.config) else {
            return;
        };
        _ = write_safe(&self.path, &bytes);
    }
}

pub(crate) fn try_read_json<T: std::fmt::Debug + Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    let Ok(data) = std::fs::read(path) else {
        return T::default();
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

pub(crate) fn write_safe(path: &Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut temp = path.to_path_buf();
    temp.add_extension(format!("{}", rand::thread_rng().next_u32()));
    temp.add_extension("new");

    let mut temp_file = std::fs::File::create(&temp)?;

    temp_file.write_all(content)?;
    temp_file.flush()?;
    temp_file.sync_all()?;

    drop(temp_file);

    if let Err(err) = std::fs::rename(&temp, path) {
        _ = std::fs::remove_file(&temp);
        return Err(err);
    }

    Ok(())
}

/// Carries groups created before upstream moved group membership onto the instance itself.
///
/// The old layout kept the group list in `interface.json` plus a list of `(InstanceID, group id)`
/// assignments. Ordering and collapsed state still live in the interface config, so they move
/// across immediately; group membership now lives on the instance and has to go through the
/// backend.
///
/// `InstanceID` is a runtime slab index, so an assignment can only be applied once that instance
/// has actually loaded, and instances arrive one at a time. Each assignment is therefore applied
/// and dropped as its instance shows up, and the legacy group list is only cleared once every
/// assignment has been consumed — clearing it up front would strip the groups off every instance
/// that had not loaded yet.
pub fn migrate_instance_group(instance_id: InstanceID, backend_handle: &BackendHandle, cx: &mut App) {
    if InterfaceConfig::get(cx).instance_groups.is_empty() {
        return;
    }

    let config = InterfaceConfig::get_mut(cx);

    // Ordering and collapsed state don't depend on any instance, so they only need doing once
    if !config.migrated_instance_group_order {
        let legacy_groups = config.instance_groups.clone();
        for group in &legacy_groups {
            let name = Arc::<str>::from(group.name.as_str());
            if !config.instance_group_order.iter().any(|existing| *existing == name) {
                config.instance_group_order.push(name);
            }
            if group.collapsed {
                config.instance_groups_closed.insert(SharedString::from(group.name.clone()));
            }
        }
        config.migrated_instance_group_order = true;
        log::info!("Migrated {} legacy instance groups", legacy_groups.len());
    }

    let Some(position) = config.instance_group_assignments.iter().position(|(id, _)| *id == instance_id) else {
        return;
    };
    let (_, group_id) = config.instance_group_assignments.remove(position);
    let group = config
        .instance_groups
        .iter()
        .find(|group| group.id == group_id)
        .map(|group| group.name.clone());

    if config.instance_group_assignments.is_empty() {
        config.instance_groups.clear();
    }

    InterfaceConfig::force_save(cx);

    if let Some(group) = group {
        backend_handle.send(MessageToBackend::MoveInstanceToGroup {
            instance_id,
            group: group.as_str().into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_before_content_labels_gets_the_defaults() {
        let config: InterfaceConfig =
            serde_json::from_str(r#"{"preferred_add_content_source":"modrinth","instance_mods_sort_key":"filename"}"#)
                .unwrap();

        for folder in [
            ContentFolder::Mods,
            ContentFolder::ResourcePacks,
            ContentFolder::Shaders,
            ContentFolder::Plugins,
        ] {
            let display = config.content_list_display(folder);
            assert_eq!(display.view_mode, ContentListViewMode::Standard);
            assert!(display.show_icons);
            assert!(display.icons_visible());
        }
        assert_eq!(config.content_categories, default_content_categories());
    }

    #[test]
    fn browsing_starts_on_the_instance_version_even_if_the_old_toggle_was_off() {
        let config: InterfaceConfig =
            serde_json::from_str(r#"{"preferred_add_content_source":"modrinth","content_filter_version":false}"#)
                .unwrap();
        assert!(config.content_filter_instance_version);

        let config: InterfaceConfig = serde_json::from_str(
            r#"{"preferred_add_content_source":"modrinth","content_filter_instance_version":false}"#,
        )
        .unwrap();
        assert!(!config.content_filter_instance_version);
    }

    #[test]
    fn display_is_kept_per_list() {
        let mut config: InterfaceConfig = serde_json::from_str(r#"{"preferred_add_content_source":"file"}"#).unwrap();
        config.content_list_display_mut(ContentFolder::Plugins).view_mode = ContentListViewMode::Compact;
        config.content_list_display_mut(ContentFolder::ResourcePacks).show_icons = false;

        let config: InterfaceConfig = serde_json::from_slice(&serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(config.content_list_display(ContentFolder::Plugins).is_compact());
        assert!(!config.content_list_display(ContentFolder::Mods).is_compact());
        assert!(!config.content_list_display(ContentFolder::ResourcePacks).icons_visible());
        assert!(config.content_list_display(ContentFolder::Mods).icons_visible());
    }

    #[test]
    fn compact_rows_never_show_icons() {
        let display = ContentListDisplay {
            view_mode: ContentListViewMode::Compact,
            show_icons: true,
        };
        assert!(!display.icons_visible());
    }

    #[test]
    fn bad_category_entries_are_dropped_alone() {
        let config: InterfaceConfig = serde_json::from_str(
            r##"{"preferred_add_content_source":"modrinth","content_categories":[
                {"id":"library","color":"#64748b"},{"color":"#ffffff"},{"id":"QoL","color":"nonsense"},7]}"##,
        )
        .unwrap();
        assert_eq!(
            config.content_categories,
            vec![
                ContentCategory {
                    id: "library".into(),
                    color: Some(HexColor::new(0x64748b)),
                },
                ContentCategory {
                    id: "QoL".into(),
                    color: None,
                },
            ]
        );
    }

    #[test]
    fn deleting_every_category_sticks() {
        let config: InterfaceConfig =
            serde_json::from_str(r#"{"preferred_add_content_source":"modrinth","content_categories":[]}"#).unwrap();
        assert!(config.content_categories.is_empty());

        let config: InterfaceConfig =
            serde_json::from_str(r#"{"preferred_add_content_source":"modrinth","content_categories":"what"}"#).unwrap();
        assert_eq!(config.content_categories, default_content_categories());
    }
}
