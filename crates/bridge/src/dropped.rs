//! Files dropped onto the launcher from outside it, and linking content to the project it came
//! from by hand.

use std::{path::Path, sync::Arc};

use schema::{content::ContentSource, loader::Loader, server::ServerPlatform};
use ustr::Ustr;

use crate::instance::{ContentFolder, ContentSummary, InstanceID};

/// A Modrinth or CurseForge project, as far as showing it goes.
#[derive(Debug, Clone)]
pub struct ResolvedProject {
    pub source: ContentSource,
    pub title: Arc<str>,
    pub icon_url: Option<Arc<str>>,
}

/// What a dropped file turned out to be.
#[derive(Debug, Clone)]
pub struct DroppedFileInfo {
    pub path: Arc<Path>,
    pub filename: Arc<str>,
    pub hash: [u8; 20],
    pub summary: Arc<ContentSummary>,
    /// The project the file is a published build of, found by its hash, or linked earlier.
    pub project: Option<ResolvedProject>,
    /// The build's own name on the site it came from, such as `mc1.21.1-0.6.13`.
    pub version_name: Option<Arc<str>>,
    /// Which folder it installs into, if that can be told.
    pub content_folder: Option<ContentFolder>,
    /// The Minecraft versions and loaders the build is for, newest version last, which a new
    /// instance for it starts on.
    pub game_versions: Arc<[Ustr]>,
    pub loaders: Arc<[Loader]>,
}

/// Where dropped files go.
#[derive(Debug, Clone)]
pub enum DropTarget {
    Instance(InstanceID),
    NewInstance {
        name: Arc<str>,
        minecraft_version: Ustr,
        loader: Loader,
    },
    NewServer {
        name: Arc<str>,
        minecraft_version: Ustr,
        platform: ServerPlatform,
        eula_accepted: bool,
    },
}

#[derive(Debug, Clone)]
pub struct DroppedFile {
    pub path: Arc<Path>,
    /// The project to record the file as coming from. `Manual` leaves it to be looked up by hash.
    pub source: ContentSource,
    /// Where it goes if its type can't be told from its contents, such as the tab it was dropped on.
    pub fallback_folder: Option<ContentFolder>,
    /// Existing installed file being replaced in place (same filename), if any.
    pub replace_old: Option<Arc<Path>>,
}
