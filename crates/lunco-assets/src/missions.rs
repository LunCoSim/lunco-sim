//! Embedded mission definitions — files under `assets/missions/`.
//!
//! Why this lives HERE: `lunco-assets` owns every asset interaction. Mission
//! data that must be present at compile time on wasm (no filesystem) is baked in
//! with `include_dir!` and handed to consumers by basename. DROP A file in
//! `assets/missions/`, rebuild, and it's reachable — no code edit here.

use include_dir::{include_dir, Dir};

/// Bundled mission tree. Baked at compile time — rebuild after editing files
/// under `assets/missions/`.
static MISSIONS_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/missions");

/// One mission file's source by basename (e.g. `"artemis_2_mission.usda"`), or `None`.
/// Case-sensitive; works for any embedded mission file (`.json`, `.usda`, …).
pub fn mission_source(filename: &str) -> Option<&'static str> {
    MISSIONS_DIR.get_file(filename).and_then(|f| f.contents_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_mission_present() {
        // A mission is USD, and only USD: nothing under `assets/missions/`
        // describes a download. Obtaining vectors is a declared dataset
        // (`crates/lunco-celestial-ephemeris/Assets.toml`), owned by
        // `crate::datasets`.
        assert!(mission_source("artemis_2_mission.usda").is_some());
        assert!(mission_source("DoesNotExist.json").is_none());
    }
}
