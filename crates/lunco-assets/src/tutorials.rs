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
    fn learning_paths_parse_as_json() {
        let v: serde_json::Value = serde_json::from_str(learning_paths_json())
            .expect("learning_paths.json must be valid JSON");
        assert!(v
            .get("paths")
            .and_then(|p| p.as_array())
            .is_some_and(|a| !a.is_empty()));
    }

    /// Every track manifest parses, and every lesson it names actually resolves.
    ///
    /// A manifest is data loaded at runtime, so a typo'd `script` path is invisible
    /// to the compiler and shows up as a lesson that opens to nothing. Enumerated
    /// from the embedded tree rather than a hardcoded list, so a NEW track is
    /// covered the moment it exists — the failure mode being guarded here is
    /// precisely a track that nothing looks at.
    #[test]
    fn every_track_manifest_resolves_its_scripts() {
        let mut manifests = 0;
        for dir in TUTORIALS.dirs() {
            let Some(f) = dir.get_file(dir.path().join("tutorials.json")) else {
                continue; // not a track (e.g. a shared asset dir)
            };
            let track = dir.path().display();
            let src = f.contents_utf8().expect("manifest is utf8");
            let metas: Vec<serde_json::Value> = serde_json::from_str(src)
                .unwrap_or_else(|e| panic!("tutorials/{track}/tutorials.json is invalid: {e}"));
            assert!(
                !metas.is_empty(),
                "tutorials/{track}/tutorials.json is empty"
            );
            manifests += 1;

            for m in metas {
                let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("<no id>");
                let script = m
                    .get("script")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("tutorials/{track}: '{id}' has no script"));
                assert!(
                    TUTORIALS.get_file(script).is_some(),
                    "tutorials/{track}: '{id}' names script '{script}', which does not exist"
                );
            }
        }
        assert!(
            manifests >= 2,
            "expected at least the basic + sandbox tracks, found {manifests}"
        );
    }
}
