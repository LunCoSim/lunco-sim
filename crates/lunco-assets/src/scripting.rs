//! Embedded rhai scripting assets — the prelude, built-in tool libraries, and
//! example scenarios authored under `assets/scripting/`.
//!
//! Why this lives HERE: `lunco-assets` owns every asset interaction. The
//! scripting substrate needs these files at compile time on EVERY target —
//! wasm has no filesystem, so a runtime scan of `assets/scripting/` is
//! impossible — so they're baked in with `include_dir!` and handed to consumers
//! as `(file_stem, source)` pairs. DROP A `.rhai` in the matching subdir,
//! rebuild, and it's picked up automatically: no Rust edit in either crate.
//!
//! Three layers, each its own flat directory:
//!   - `prelude/`  — always-on helpers, merged into one flat namespace.
//!   - `tools/`    — namespaced `name::fn(...)` tool libraries (name = stem).
//!   - `examples/` — sample scenarios, for docs / the catalog / the parse test.

use include_dir::{include_dir, Dir};

/// Prelude topic files — always-on rhai helpers.
static PRELUDE: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/scripting/prelude");
/// Built-in tool libraries — namespaced `name::fn(...)` bundles.
static TOOLS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/scripting/tools");
/// Example scenarios — used by docs / the parse test / the catalog.
static EXAMPLES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/scripting/examples");

/// Every top-level `*.rhai` in `dir` as `(file_stem, source)`, sorted by stem so
/// merge/iteration order is deterministic across builds and targets. Non-UTF8
/// files are skipped (nothing legitimately authored here is binary).
fn rhai_files(dir: &'static Dir<'static>) -> Vec<(&'static str, &'static str)> {
    let mut files: Vec<(&'static str, &'static str)> = dir
        .files()
        .filter(|f| f.path().extension().and_then(|e| e.to_str()) == Some("rhai"))
        .filter_map(|f| Some((f.path().file_stem()?.to_str()?, f.contents_utf8()?)))
        .collect();
    files.sort_by_key(|(stem, _)| *stem);
    files
}

/// Prelude topic files (`assets/scripting/prelude/*.rhai`) as `(stem, source)`.
pub fn prelude_files() -> Vec<(&'static str, &'static str)> {
    rhai_files(&PRELUDE)
}

/// Built-in tool libraries (`assets/scripting/tools/*.rhai`) as `(stem, source)`.
pub fn tool_libraries() -> Vec<(&'static str, &'static str)> {
    rhai_files(&TOOLS)
}

/// Example scenarios (`assets/scripting/examples/*.rhai`) as `(stem, source)`.
pub fn examples() -> Vec<(&'static str, &'static str)> {
    rhai_files(&EXAMPLES)
}

/// One example scenario's source by file stem (e.g. `"mission_plan"`), or `None`.
pub fn example(stem: &str) -> Option<&'static str> {
    EXAMPLES
        .get_file(format!("{stem}.rhai"))
        .and_then(|f| f.contents_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_scripting_dirs_are_non_empty_and_sorted() {
        for (label, files) in [
            ("prelude", prelude_files()),
            ("tools", tool_libraries()),
            ("examples", examples()),
        ] {
            assert!(!files.is_empty(), "{label} embedded empty");
            let mut sorted = files.clone();
            sorted.sort_by_key(|(s, _)| *s);
            assert_eq!(files, sorted, "{label} not sorted by stem");
        }
        // Known built-ins are present (guards a broken move / path).
        let tool_names: Vec<_> = tool_libraries().into_iter().map(|(n, _)| n).collect();
        for t in ["formation", "survey", "debug_viz"] {
            assert!(tool_names.contains(&t), "tool {t} missing: {tool_names:?}");
        }
        assert!(example("mission_plan").is_some());
        assert!(example("nope").is_none());
    }
}
