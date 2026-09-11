//! Authored tutorial content — files under `assets/tutorials/`.
//!
//! Why this lives HERE: `lunco-assets` owns every asset interaction, INCLUDING
//! the native-disk-vs-wasm-embed policy. Consumers ask this crate for a
//! tutorial's text and never touch `include_str!`/the filesystem themselves.
//!
//! Two access shapes, by how the data is used:
//! - [`tutorial_catalog_json`] — the menu's presentation catalog. Native builds
//!   reread it so menu edits do not require a Rust rebuild; wasm uses the
//!   embedded copy.
//! - [`tutorial_source`] — a rhai orchestrator that a user may want to
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

/// Read the menu catalog for authored tutorials.
///
/// The catalog contains only presentation and launch references. It is not a
/// runtime state store and is not used for change detection. Native builds
/// prefer the adjacent file so a menu edit is visible after restarting the app;
/// packaged and wasm builds use the embedded copy.
pub fn tutorial_catalog_json() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = crate::assets_dir().join("tutorials/catalog.json");
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
    }
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/tutorials/catalog.json"
    ))
    .to_string()
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
    if !crate::asset_path::is_safe_relative_path(rel) {
        return None;
    }
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

/// Every embedded tutorial `.rhai`, as `(relative path, source)` — recursive, so it
/// spans every track shipped from `assets/` (`basic/…`, `sandbox/…`, `lunica/…`).
/// A Twin's own lessons are NOT here: they load from `<twin>/sim/tutorials/`, so a
/// track like the Summer Space School is enumerated by that Twin, not by this.
///
/// The EMBEDDED copies specifically: this is the enumerator, and there is no
/// on-disk walk behind it, because its purpose is to let a test hold every tutorial
/// at once (see `lunco-scripting/tests/prelude_parses.rs`). A rhai asset is
/// invisible to `cargo check` — a syntax error in one surfaces only when a student
/// launches that lesson — so being able to enumerate them is what makes them
/// testable. For LOADING one, use [`tutorial_source`], which prefers the on-disk
/// file so live edits replay without a rebuild.
pub fn tutorial_files() -> Vec<(String, String)> {
    fn walk(dir: &'static Dir<'static>, out: &mut Vec<(String, String)>) {
        for f in dir.files() {
            if f.path().extension().and_then(|e| e.to_str()) != Some("rhai") {
                continue;
            }
            if let Some(src) = f.contents_utf8() {
                out.push((f.path().display().to_string(), src.to_string()));
            }
        }
        for d in dir.dirs() {
            walk(d, out);
        }
    }
    let mut out = Vec::new();
    walk(&TUTORIALS, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tutorial_catalog_is_valid_and_references_authored_scripts() {
        let value: serde_json::Value = serde_json::from_str(&tutorial_catalog_json())
            .expect("tutorial catalog must be valid JSON");
        let entries = value
            .get("tutorials")
            .and_then(serde_json::Value::as_array)
            .expect("tutorial catalog must contain a tutorials array");
        assert!(!entries.is_empty());
        for entry in entries {
            let source = entry
                .get("source_asset")
                .and_then(serde_json::Value::as_str)
                .expect("every tutorial needs a source_asset");
            assert!(source.starts_with("lunco://tutorials/"));
            let relative = source.trim_start_matches("lunco://tutorials/");
            assert!(tutorial_source(relative).is_some(), "missing {source}");
        }
    }

    #[test]
    fn tutorial_source_rejects_root_escape() {
        assert!(tutorial_source("../Cargo.toml").is_none());
        assert!(tutorial_source("sandbox/../../Cargo.toml").is_none());
        assert!(tutorial_source(r"sandbox\..\Cargo.toml").is_none());
    }
}
