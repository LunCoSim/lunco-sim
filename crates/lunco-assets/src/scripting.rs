//! rhai scripting assets — the prelude, built-in tool libraries, and example
//! scenarios authored under `assets/scripting/`.
//!
//! Why this lives HERE: `lunco-assets` owns every asset interaction. Every set
//! is EMBEDDED with `include_dir!` (wasm has no filesystem, and an installed
//! binary may run without an `assets/` tree beside it), but the PRELUDE is
//! loaded **from disk at startup** on native when `assets/scripting/prelude/`
//! exists — edit a helper, restart, no rebuild (policy → script, no Rust
//! edit). The embedded copy is the always-works fallback and the wasm source
//! of truth. DROP A `.rhai` in the matching subdir and it's picked up at the
//! next launch when running from the repo, at the next rebuild everywhere else.
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
/// Built-in rhai POLICY snippets (RBAC/authorization/control-authority) registered
/// as `lunco_hooks` at startup — the `policy→rhai` decision surface.
static POLICY: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/scripting/policy");
/// Bundled runtime scenarios — the guidance/mission scripts a scene loads at
/// startup (e.g. lander auto-land). Distinct from `examples/`: these are shipped
/// behaviour, not documentation samples, and live alongside the scene assets.
static SCENARIOS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/scenarios");

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

/// Prelude topic files as `(stem, source)`. Native: read from
/// `assets/scripting/prelude/*.rhai` at call time (each engine build — i.e. app
/// start), so prelude edits need only a restart; when the directory is absent
/// or empty (installed build, odd CWD) the embedded copy serves. wasm: always
/// embedded. A DISK prelude that fails to PARSE is handled by the consumer
/// (`compile_prelude` falls back to [`embedded_prelude_files`] so a broken
/// edit can never brick startup).
pub fn prelude_files() -> Vec<(String, String)> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(files) = disk_rhai_files(&crate::assets_dir().join("scripting/prelude")) {
        return files;
    }
    embedded_prelude_files()
}

/// The compiled-in prelude (the fallback + wasm source of truth).
pub fn embedded_prelude_files() -> Vec<(String, String)> {
    rhai_files(&PRELUDE)
        .into_iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect()
}

/// Every top-level `*.rhai` in the on-disk `dir`, sorted by stem (the same
/// deterministic order [`rhai_files`] gives the embedded sets). `None` when the
/// directory is missing, unreadable, or holds no `.rhai` — callers fall back to
/// the embedded copy rather than silently running with an empty prelude.
#[cfg(not(target_arch = "wasm32"))]
fn disk_rhai_files(dir: &std::path::Path) -> Option<Vec<(String, String)>> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut files: Vec<(String, String)> = entries
        .filter_map(|e| {
            let p = e.ok()?.path();
            if p.extension().and_then(|x| x.to_str()) != Some("rhai") {
                return None;
            }
            Some((
                p.file_stem()?.to_str()?.to_string(),
                std::fs::read_to_string(&p).ok()?,
            ))
        })
        .collect();
    if files.is_empty() {
        return None;
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Some(files)
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

/// Bundled runtime scenarios (`assets/scenarios/*.rhai`) as `(stem, source)`.
pub fn scenarios() -> Vec<(&'static str, &'static str)> {
    rhai_files(&SCENARIOS)
}

/// One bundled scenario's source by file stem (e.g. `"lander_subsystems"`).
pub fn scenario(stem: &str) -> Option<&'static str> {
    SCENARIOS
        .get_file(format!("{stem}.rhai"))
        .and_then(|f| f.contents_utf8())
}

/// Built-in policy snippets (`assets/scripting/policy/*.rhai`) as `(stem, source)`.
pub fn policies() -> Vec<(&'static str, &'static str)> {
    rhai_files(&POLICY)
}

/// One built-in policy's source by file stem (e.g. `"control_authority"`).
pub fn policy(stem: &str) -> Option<&'static str> {
    POLICY
        .get_file(format!("{stem}.rhai"))
        .and_then(|f| f.contents_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_scripting_dirs_are_non_empty_and_sorted() {
        for (label, files) in [
            ("tools", tool_libraries()),
            ("examples", examples()),
            ("scenarios", scenarios()),
            ("policy", policies()),
        ] {
            assert!(!files.is_empty(), "{label} embedded empty");
            let mut sorted = files.clone();
            sorted.sort_by_key(|(s, _)| *s);
            assert_eq!(files, sorted, "{label} not sorted by stem");
        }
        // Prelude: both the embedded fallback and the (possibly disk-loaded)
        // active set must be non-empty and stem-sorted.
        for (label, files) in
            [("prelude-embedded", embedded_prelude_files()), ("prelude-active", prelude_files())]
        {
            assert!(!files.is_empty(), "{label} empty");
            let mut sorted = files.clone();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(files, sorted, "{label} not sorted by stem");
        }
        // Known built-ins are present (guards a broken move / path).
        let tool_names: Vec<_> = tool_libraries().into_iter().map(|(n, _)| n).collect();
        for t in ["formation", "survey", "debug_viz"] {
            assert!(tool_names.contains(&t), "tool {t} missing: {tool_names:?}");
        }
        assert!(example("mission_plan").is_some());
        assert!(example("nope").is_none());
        // The lander auto-land guidance scenario must be present and enumerable.
        assert!(scenario("lander_subsystems").is_some());
        assert!(scenario("nope").is_none());
    }
}
