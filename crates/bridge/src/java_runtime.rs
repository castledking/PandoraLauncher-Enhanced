use std::sync::Arc;

/// One of the Java runtimes Mojang publishes for the platform the launcher is on, as
/// offered by the runtime override picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaRuntimeEntry {
    /// The id from Mojang's runtime manifest, such as `java-runtime-gamma`. This is what
    /// gets written down as the instance's override and what the runtime is fetched under.
    pub component: Arc<str>,
    /// The Java version the runtime ships, such as `17.0.8`, for telling two runtimes
    /// apart in the picker.
    pub version: Arc<str>,
    /// Whether the runtime is already unpacked on disk, so picking it doesn't need a
    /// download. The launcher still verifies it either way.
    pub downloaded: bool,
}
