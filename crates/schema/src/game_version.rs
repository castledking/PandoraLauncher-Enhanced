//! Relationships between Minecraft release numbers, for offering content made for a neighbouring
//! release when there's none for the one being played.

/// Releases of the same feature release as `version` and older than it, nearest first. Content made
/// for one of them often runs on `version` too (a 1.21 mod on 1.21.1), but not always (1.20.2 broke
/// most 1.20.1 mods), so the user should be told before one is used. Snapshots have none.
pub fn earlier_releases_in_family(version: &str) -> Vec<String> {
    let parts: Vec<&str> = version.split('.').collect();
    let (Some(major), Some(minor)) = (parts.first(), parts.get(1)) else {
        return Vec::new();
    };
    if !parts.iter().all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())) {
        return Vec::new();
    }
    let Some(patch) = parts.get(2).and_then(|patch| patch.parse::<u32>().ok()) else {
        return Vec::new();
    };
    if parts.len() > 3 {
        return Vec::new();
    }

    let mut earlier: Vec<String> = (1..patch).rev().map(|patch| format!("{major}.{minor}.{patch}")).collect();
    earlier.push(format!("{major}.{minor}"));
    earlier
}

/// The release among `available` that content for `version` would best be borrowed from: the
/// nearest earlier one of the same feature release.
pub fn closest_earlier_release<'a>(version: &str, available: impl IntoIterator<Item = &'a str> + Clone) -> Option<&'a str> {
    earlier_releases_in_family(version)
        .into_iter()
        .find_map(|candidate| available.clone().into_iter().find(|available| *available == candidate))
}

/// Whether `version` looks like a full release rather than a snapshot, pre-release or candidate.
pub fn is_release(version: &str) -> bool {
    !version.is_empty() && version.split('.').all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_release_looks_back_to_its_feature_release() {
        assert_eq!(earlier_releases_in_family("1.21.1"), ["1.21"]);
        assert_eq!(earlier_releases_in_family("1.20.4"), ["1.20.3", "1.20.2", "1.20.1", "1.20"]);
        assert_eq!(earlier_releases_in_family("26.1.2"), ["26.1.1", "26.1"]);
    }

    #[test]
    fn feature_releases_and_snapshots_have_nothing_earlier() {
        assert!(earlier_releases_in_family("1.21").is_empty());
        assert!(earlier_releases_in_family("26.1").is_empty());
        assert!(earlier_releases_in_family("24w14a").is_empty());
        assert!(earlier_releases_in_family("1.21.5-pre1").is_empty());
        assert!(earlier_releases_in_family("1.21.1.1").is_empty());
        assert!(earlier_releases_in_family("").is_empty());
    }

    #[test]
    fn the_nearest_available_release_is_picked() {
        let available = ["1.20", "1.20.2", "1.19.4"];
        assert_eq!(closest_earlier_release("1.20.4", available), Some("1.20.2"));
        assert_eq!(closest_earlier_release("1.20.1", available), Some("1.20"));
        assert_eq!(closest_earlier_release("1.21.1", available), None);
    }

    #[test]
    fn releases_are_told_apart_from_snapshots() {
        assert!(is_release("1.21.1"));
        assert!(is_release("26.1"));
        assert!(!is_release("24w14a"));
        assert!(!is_release("1.21-rc1"));
        assert!(!is_release(""));
    }
}
