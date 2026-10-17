//! The Vanilla, Performance and Expanded instances Pandora sets up in their own group.
//!
//! They used to be one hidden instance the quickplay page swapped between presets. They are now
//! ordinary instances, created once: deleting one, or the whole group, is permanent, since the
//! frontend only asks for them the first time it starts.

use std::sync::Arc;

use bridge::{
    install::{ContentInstall, InstallTarget},
    instance::InstanceID,
    message::EmbeddedOrRaw,
    modal_action::ModalAction,
};
use rustc_hash::FxHashSet;
use schema::{
    loader::Loader,
    quickplay::{self, QuickplayPreset},
};

use crate::{BackendState, metadata::items::MinecraftVersionManifestMetadataItem, quickplay_presets};

impl BackendState {
    /// See [`bridge::message::MessageToBackend::CreatePresetInstances`].
    pub async fn create_preset_instances(self: &Arc<Self>, legacy_preset: QuickplayPreset) -> bool {
        let latest_release = match self.meta.fetch(MinecraftVersionManifestMetadataItem).await {
            Ok(manifest) => manifest.latest.release,
            Err(err) => {
                log::warn!("Unable to set up the preset instances, no version manifest: {err:?}");
                return false;
            },
        };

        let mut existing = FxHashSet::default();
        let mut carried_over = None;
        {
            let mut state = self.instance_state.write();
            for instance in state.instances.iter_mut() {
                if let Some(preset) = instance.configuration.get().preset {
                    existing.insert(preset);
                }
            }

            if let Some(legacy) = state.instances.iter_mut().find(|i| i.name == quickplay::INSTANCE_NAME) {
                let preset = legacy_instance_preset(legacy.configuration.get().loader, legacy_preset);
                if !existing.contains(&preset) {
                    // Its worlds and settings are kept by turning it into that preset's instance.
                    // The mods are already in, swapped in when the preset was last launched
                    legacy.configuration.modify(|configuration| {
                        configuration.group = Some(quickplay::GROUP_NAME.into());
                        configuration.preset = Some(preset);
                        configuration.preset_pending = false;
                        configuration.instance_fallback_icon = Some(preset.icon().into());
                    });
                    existing.insert(preset);
                    carried_over = Some((legacy.id, preset));
                }
            }
        }

        if let Some((id, preset)) = carried_over {
            log::info!("Carrying the old quickplay instance over as {}", preset.name());
            let name = self.unused_instance_name(preset.name());
            self.rename_instance(id, &name).await;
        }

        // Newest instances are listed first, so creating them backwards leaves them in order
        for preset in QuickplayPreset::ALL.into_iter().rev() {
            if existing.contains(&preset) {
                continue;
            }

            let loader = quickplay_presets::loader(preset);
            let name = self.unused_instance_name(preset.name());
            let icon = Some(EmbeddedOrRaw::Embedded(preset.icon().into()));
            let path = self.create_instance_configured(&name, latest_release.as_str(), loader, icon, |configuration| {
                configuration.group = Some(quickplay::GROUP_NAME.into());
                configuration.preset = Some(preset);
                configuration.preset_pending = loader != Loader::Vanilla;
            });
            let Some(path) = path else {
                return false;
            };
            self.load_instance_from_path(&path, false, false);
        }

        true
    }

    /// Whether launching the instance has to install its preset's mods first.
    pub fn has_pending_preset(&self, id: InstanceID) -> bool {
        let mut state = self.instance_state.write();
        let Some(instance) = state.instances.get_mut(id) else {
            return false;
        };
        let configuration = instance.configuration.get();
        configuration.preset_pending && configuration.preset.is_some()
    }

    /// Installs a preset instance's mods ahead of its first launch.
    ///
    /// Returns false if that failed or was cancelled, in which case the reason is already on
    /// `modal_action` and the launch shouldn't go ahead. The install is tried again next launch.
    pub async fn install_pending_preset(self: &Arc<Self>, id: InstanceID, modal_action: &ModalAction) -> bool {
        let (preset, loader, minecraft_version) = {
            let mut state = self.instance_state.write();
            let Some(instance) = state.instances.get_mut(id) else {
                return true;
            };
            let configuration = instance.configuration.get();
            let Some(preset) = configuration.preset.filter(|_| configuration.preset_pending) else {
                return true;
            };
            (preset, configuration.loader, configuration.minecraft_version)
        };

        // Someone who has moved the instance to another loader has no use for the preset's mods
        if loader == quickplay_presets::loader(preset) && loader != Loader::Vanilla {
            let files =
                quickplay_presets::resolve_installs(preset, minecraft_version, self.meta.clone(), modal_action).await;
            if modal_action.has_requested_cancel() {
                return false;
            }

            let install = ContentInstall {
                target: InstallTarget::Instance(id),
                loader,
                minecraft_version,
                files,
            };
            self.install_content(install, modal_action.clone()).await;

            if modal_action.get_error_message().is_some() || modal_action.has_requested_cancel() {
                return false;
            }
        }

        if let Some(instance) = self.instance_state.write().instances.get_mut(id) {
            instance.configuration.modify(|configuration| configuration.preset_pending = false);
            self.send.send(instance.create_modify_message());
        }

        true
    }

    /// `base`, or `base (1)`, `base (2)`... if an instance already has that name, so a preset
    /// never collides with an instance someone made themselves.
    fn unused_instance_name(&self, base: &str) -> String {
        let state = self.instance_state.read();
        let taken = |name: &str| state.instances.iter().any(|instance| instance.name.as_str() == name);
        if !taken(base) {
            return base.to_string();
        }
        (1..).map(|i| format!("{base} ({i})")).find(|name| !taken(name)).unwrap()
    }
}

/// Which preset the old hidden quickplay instance holds. It swapped its loader and mods for the
/// preset picked last, and the two Fabric presets can only be told apart by that pick.
fn legacy_instance_preset(loader: Loader, last_picked: QuickplayPreset) -> QuickplayPreset {
    match (loader, last_picked) {
        (Loader::Vanilla, _) => QuickplayPreset::Vanilla,
        (_, QuickplayPreset::Vanilla) => QuickplayPreset::Performance,
        (_, picked) => picked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vanilla_legacy_instance_is_the_vanilla_preset() {
        assert_eq!(
            legacy_instance_preset(Loader::Vanilla, QuickplayPreset::Expanded),
            QuickplayPreset::Vanilla
        );
    }

    #[test]
    fn a_modded_legacy_instance_follows_the_last_pick() {
        assert_eq!(
            legacy_instance_preset(Loader::Fabric, QuickplayPreset::Expanded),
            QuickplayPreset::Expanded
        );
        assert_eq!(
            legacy_instance_preset(Loader::Fabric, QuickplayPreset::Performance),
            QuickplayPreset::Performance
        );
    }

    #[test]
    fn a_modded_legacy_instance_is_never_vanilla() {
        // Picking Vanilla without launching it leaves the Fabric mods from the last launch in place
        assert_eq!(
            legacy_instance_preset(Loader::Fabric, QuickplayPreset::Vanilla),
            QuickplayPreset::Performance
        );
    }
}
