use std::path::PathBuf;

#[derive(Default, Debug)]
pub struct ImportFromOtherLaunchers {
    pub imports: enum_map::EnumMap<OtherLauncher, Option<ImportFromOtherLauncher>>,
}

#[derive(Debug)]
pub struct ImportFromOtherLauncher {
    pub can_import_accounts: bool,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, enum_map::Enum)]
pub enum OtherLauncher {
    Prism,
    Modrinth,
    MultiMC,
}
