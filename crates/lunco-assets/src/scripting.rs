//! rhai scripting assets — the prelude, built-in tool libraries, and example
//! scenarios authored under `assets/scripting/`.
//!
//! Why this lives HERE: `lunco-assets` owns every asset interaction. Every set
//! is EMBEDDED with `include_dir!` (wasm has no filesystem, and an installed
//! binary may run without an `assets/` tree beside it), but the PRELUDE is
//! loaded **from disk at startup** on native when the corresponding
//! `assets/scripting/*/` directory exists — edit a helper or policy, restart,
//! no Rust rebuild. The embedded copies are the packaged source of truth for
//! wasm and installed builds without an asset tree. Once a live source
//! directory is selected, its contents are authoritative and parse failures
//! are surfaced; consumers do not silently switch to stale embedded policy.
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
/// Built-in rhai POLICY snippets registered as `lunco_hooks` at startup — the
/// `policy→rhai` decision surface.
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

/// Prelude topic files as `(stem, source)`. Native checkouts read
/// `assets/scripting/prelude/*.rhai` at call time (each engine build — i.e. app
/// start), so prelude edits need only a restart. Packaged builds and wasm use
/// the compiled-in source because no editable asset directory is part of the
/// runtime. A present native directory is authoritative: unreadable, empty,
/// or malformed source is an error, not a switch to another generation.
pub fn prelude_files() -> Result<Vec<(String, String)>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(files) = disk_rhai_files(&crate::assets_dir().join("scripting/prelude"))? {
        return Ok(files);
    }
    Ok(embedded_prelude_files())
}

/// The compiled-in prelude used by packaged and wasm builds.
pub fn embedded_prelude_files() -> Vec<(String, String)> {
    rhai_files(&PRELUDE)
        .into_iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect()
}

/// Every top-level `*.rhai` in the on-disk `dir`, sorted by stem (the same
/// deterministic order [`rhai_files`] gives the embedded sets). `None` means
/// that the directory is not present, which selects the packaged source. Any
/// other filesystem condition is an error so a broken editable policy cannot
/// be hidden by another source set.
#[cfg(not(target_arch = "wasm32"))]
fn disk_rhai_files(dir: &std::path::Path) -> Result<Option<Vec<(String, String)>>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "cannot read Rhai asset directory {}: {error}",
                dir.display()
            ));
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| {
                format!(
                    "cannot enumerate Rhai asset directory {}: {error}",
                    dir.display()
                )
            })?
            .path();
        if path.extension().and_then(|x| x.to_str()) != Some("rhai") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("Rhai asset filename is not valid UTF-8: {}", path.display()))?
            .to_string();
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read Rhai asset {}: {error}", path.display()))?;
        files.push((stem, source));
    }
    if files.is_empty() {
        return Err(format!(
            "Rhai asset directory {} contains no .rhai files",
            dir.display()
        ));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Some(files))
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

/// Compiled-in policy snippets as owned strings.
pub fn embedded_policy_files() -> Vec<(String, String)> {
    policies()
        .into_iter()
        .map(|(stem, source)| (stem.to_string(), source.to_string()))
        .collect()
}

/// Active policy snippets for native startup.
///
/// A repository checkout reads the editable policy files so changing Rhai does
/// not require a Rust rebuild. If no live policy directory exists, packaged
/// builds use the compiled-in source. A present live directory remains the
/// authoritative source; registration reports its parse errors instead of
/// replacing it with stale embedded code.
pub fn policy_files() -> Result<Vec<(String, String)>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(files) = disk_rhai_files(&crate::assets_dir().join("scripting/policy"))? {
        return Ok(files);
    }
    Ok(embedded_policy_files())
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
        // Both the embedded packaged source and the active source set must be
        // non-empty and stem-sorted.
        for (label, files) in [
            ("prelude-embedded", embedded_prelude_files()),
            (
                "prelude-active",
                prelude_files().expect("active prelude source"),
            ),
        ] {
            assert!(!files.is_empty(), "{label} empty");
            let mut sorted = files.clone();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(files, sorted, "{label} not sorted by stem");
        }
        // Known built-ins are present (guards a broken move / path).
        let tool_names: Vec<_> = tool_libraries().into_iter().map(|(n, _)| n).collect();
        for t in ["assembly_edit", "formation", "survey", "debug_viz"] {
            assert!(tool_names.contains(&t), "tool {t} missing: {tool_names:?}");
        }
        assert!(example("mission_plan").is_some());
        assert!(example("nope").is_none());
        // The lander auto-land guidance scenario must be present and enumerable.
        assert!(scenario("lander_subsystems").is_some());
        assert!(scenario("nope").is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn editable_rhai_directory_selection_is_explicit() {
        let root = tempfile::tempdir().expect("temporary Rhai directory");
        assert!(disk_rhai_files(root.path().join("missing").as_path())
            .expect("missing directory is a packaged-source decision")
            .is_none());

        let empty = root.path().join("empty");
        std::fs::create_dir(&empty).expect("empty Rhai directory");
        let error = disk_rhai_files(&empty).expect_err("empty editable source is invalid");
        assert!(error.contains("contains no .rhai files"), "{error}");

        std::fs::write(empty.join("policy.rhai"), "fn policy() { true }")
            .expect("editable Rhai source");
        let files = disk_rhai_files(&empty)
            .expect("editable Rhai source is readable")
            .expect("directory is present");
        assert_eq!(
            files,
            [("policy".to_string(), "fn policy() { true }".to_string())]
        );
    }
}
