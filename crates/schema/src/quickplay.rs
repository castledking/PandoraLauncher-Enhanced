use serde::{Deserialize, Serialize};

/// The single hidden instance the old quickplay page switched between presets. It is carried
/// over into the preset instances below the first time they are set up.
pub const INSTANCE_NAME: &'static str = "__QUICKPLAY__";

/// The group the preset instances are created in.
pub const GROUP_NAME: &'static str = "Pandora";

#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuickplayPreset {
    #[default]
    Vanilla,
    Performance,
    Expanded,
}

impl QuickplayPreset {
    /// In the order the instances are shown.
    pub const ALL: [QuickplayPreset; 3] = [
        QuickplayPreset::Vanilla,
        QuickplayPreset::Performance,
        QuickplayPreset::Expanded,
    ];

    /// The instance name the preset is created under.
    pub const fn name(self) -> &'static str {
        match self {
            QuickplayPreset::Vanilla => "Vanilla",
            QuickplayPreset::Performance => "Performance",
            QuickplayPreset::Expanded => "Expanded",
        }
    }

    pub const fn icon(self) -> &'static str {
        match self {
            QuickplayPreset::Vanilla => "icons/box.svg",
            QuickplayPreset::Performance => "icons/rocket.svg",
            QuickplayPreset::Expanded => "icons/galaxy.svg",
        }
    }

    /// Where the preset sits in [`Self::ALL`].
    pub fn order(self) -> usize {
        Self::ALL.iter().position(|preset| *preset == self).unwrap_or(usize::MAX)
    }
}
