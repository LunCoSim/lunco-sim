//! Tutorial curriculum data — files under `assets/tutorials/`.
//!
//! Why this lives HERE: `lunco-assets` owns every asset interaction, INCLUDING
//! the native-disk-vs-wasm-embed policy. Consumers ask this crate for a
//! tutorial's text and never touch `include_str!`/the filesystem themselves.
//!
//! Two access shapes, by how the data is used:
//! - [`learning_paths_json`] — compile-time constant (parsed once into a
//!   registry); always embedded.
//! - [`lunica_tutorial_source`] — a rhai orchestrator that a user may want to
//!   **edit and replay live**. Native reads it fresh from disk each call (so an
//!   edit lands on the next launch with no rebuild); wasm (no fs) serves the
//!   embedded copy. This split is the whole reason source loading is centralised
//!   here rather than `include_str!`'d at the call site.

use include_dir::{include_dir, Dir};

/// The tutorial orchestrators (and any tutorial data), embedded at compile time.
/// On native this is the fallback when the on-disk file is missing (a packaged
/// app run outside the repo); on wasm it is the only source. Recursive, so it
/// covers per-app subdirs (`lunica/…`, `first_drive/…`, …).
static TUTORIALS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/tutorials");

/// The learning-paths curriculum as raw JSON (`assets/tutorials/learning_paths.json`).
/// The lunica Welcome panel parses this into its `LearningPathRegistry`. Edit the
/// JSON and rebuild to change the curriculum — no code edit here.
pub fn learning_paths_json() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/tutorials/learning_paths.json"
    ))
}

/// Load a tutorial orchestrator's rhai source by its path **relative to
/// `assets/tutorials/`** (e.g. `"lunica/overview.rhai"`, `"first_drive/first_drive.rhai"`).
///
/// This is the single source for EVERY tutorial in EVERY app — a tutorial is
/// just a `.rhai` scenario, so the shared launcher loads them all through here.
///
/// **Native:** reads `<`[`assets_dir`](crate::assets_dir)`>/tutorials/<rel>` from
/// disk on every call, so editing a tutorial and re-launching it replays the
/// change with no rebuild (the live-authoring path). Falls back to the embedded
/// copy when the file is absent (a packaged binary run outside the repo).
/// **wasm:** always returns the embedded copy. `None` if no such tutorial exists.
pub fn tutorial_source(rel: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = crate::assets_dir().join("tutorials").join(rel);
        if let Ok(src) = std::fs::read_to_string(&path) {
            return Some(src);
        }
        // fall through to the embedded copy
    }
    TUTORIALS
        .get_file(rel)
        .and_then(|f| f.contents_utf8())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learning_paths_parse_as_json() {
        let v: serde_json::Value = serde_json::from_str(learning_paths_json())
            .expect("learning_paths.json must be valid JSON");
        assert!(v.get("paths").and_then(|p| p.as_array()).is_some_and(|a| !a.is_empty()));
    }
}
