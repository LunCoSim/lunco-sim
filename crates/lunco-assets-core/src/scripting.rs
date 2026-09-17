//! rhai scripting assets — the prelude, built-in tool libraries, and example
//! scenarios authored under `assets/scripting/`.
//!
//! Why this lives HERE: `lunco-assets-core` owns shared asset interaction. Every set
//! is EMBEDDED with `include_dir!` (wasm has no filesystem, and an installed
//! binary may run without an `assets/` tree beside it), but the PRELUDE is
//! loaded **from disk at startup** on native when the corresponding
//! `assets/scripting/*/` directory exists — edit a helper or policy, restart,
//! no Rust rebuild. The embedded copies are the packaged source of truth for
//! wasm and installed builds without an asset tree. Once a live source
//! directory is selected, its contents are authoritative and parse failures
//! are surfaced; consumers do not silently switch to stale embedded policy.
//!
//! Native checkouts read the prelude, policy, and tool directories from disk at
//! startup; packaged and wasm builds use the embedded copies. Runtime tool
//! replacement uses the registration command and does not require a restart.
//!
//! Three layers, each its own flat directory, plus a manifest-driven policy
//! directory:
//!   - `prelude/`  — always-on helpers, merged into one flat namespace.
//!   - `tools/`    — namespaced `name::fn(...)` tool libraries (name = stem).
//!   - `examples/` — sample scenarios, for docs / the catalog / the parse test.
//!   - `policy/`   — hook implementations selected by `index.toml`; the
//!                   manifest's startup entry orchestrates their installation.

use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Component, Path};

/// Prelude topic files — always-on rhai helpers.
static PRELUDE: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/scripting/prelude");
/// Built-in tool libraries — namespaced `name::fn(...)` bundles.
static TOOLS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/scripting/tools");
/// Example scenarios — used by docs / the parse test / the catalog.
static EXAMPLES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/scripting/examples");
/// Embedded Rhai policy sources. The manifest below, rather than Rust source,
/// selects which source and entry function implements each hook at startup.
static POLICY: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/scripting/policy");
/// Authored mapping from hook ids to policy source files and entry functions.
static POLICY_MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/scripting/policy/index.toml"
));
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

/// Embedded built-in tool libraries (`assets/scripting/tools/*.rhai`) as
/// `(stem, source)`. Native startup uses [`active_tool_libraries`] so a
/// checkout can replace these sources without rebuilding Rust.
pub fn tool_libraries() -> Vec<(&'static str, &'static str)> {
    rhai_files(&TOOLS)
}

/// Active built-in tool libraries for native startup.
///
/// A checkout with an editable `assets/scripting/tools/` directory reads those
/// files at startup. That directory is authoritative: an unreadable or empty
/// directory is an error rather than a silent return to stale embedded policy.
/// Packaged and wasm builds use the embedded source because no editable asset
/// tree is available there. Runtime `RegisterToolLibrary` remains the
/// hot-reload path after startup.
pub fn active_tool_libraries() -> Result<Vec<(String, String)>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(files) = disk_rhai_files(&crate::assets_dir_abs().join("scripting/tools"))? {
        return Ok(files);
    }
    Ok(tool_libraries()
        .into_iter()
        .map(|(name, source)| (name.to_string(), source.to_string()))
        .collect())
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

/// Policy snippets embedded in the application package as `(stem, source)`.
pub fn policies() -> Vec<(&'static str, &'static str)> {
    rhai_files(&POLICY)
}

/// Authored mapping from a hook id to its policy source and entry point.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolicySpec {
    /// Hook id that receives the compiled policy.
    pub hook: String,
    /// Relative `.rhai` source path beside the manifest.
    pub source: String,
    /// Function exported by `source`.
    pub entry: String,
    /// Whether the policy is safe for convergent/replicated execution.
    #[serde(default)]
    pub deterministic: bool,
    /// Whether failure to load this policy blocks the owning seam.
    #[serde(default)]
    pub required: bool,
}

/// The authored Rhai function that installs one resolved policy set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StartupSpec {
    /// Relative `.rhai` source path beside the application manifest.
    pub source: String,
    /// Function exported by `source`.
    pub entry: String,
}

/// A policy manifest. Twin manifests may be empty and omit `startup`: optional
/// hook seams do not require an implementation merely because the scripting
/// backend exists. The application manifest must provide `startup` so its
/// broad default policy set has one explicit installation boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct PolicyManifest {
    /// The one application bootstrap function. Twin manifests omit this and
    /// contribute only policy overrides to the application's bootstrap.
    #[serde(default)]
    pub startup: Option<StartupSpec>,
    /// Policies to load in declaration order.
    #[serde(default)]
    pub policies: Vec<PolicySpec>,
}

/// One policy source after its authored file has been resolved through the
/// asset/storage boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedPolicy {
    /// Manifest metadata for the source.
    pub spec: PolicySpec,
    /// UTF-8 Rhai source.
    pub source: String,
    /// Stable asset-relative path reported by hook reflection.
    pub policy_file: String,
}

/// The resolved application bootstrap and the policy sources it receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedPolicyBundle {
    /// The startup function that receives [`policies`](Self::policies), when
    /// this is a manifest with a startup section. Twin manifests may omit it
    /// when they contain no policy overrides.
    pub startup: Option<LoadedStartup>,
    /// Policy source files selected by this manifest.
    pub policies: Vec<LoadedPolicy>,
}

/// One resolved application startup function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedStartup {
    /// Manifest metadata for the startup function.
    pub spec: StartupSpec,
    /// UTF-8 Rhai source.
    pub source: String,
    /// Stable asset-relative path reported in diagnostics.
    pub policy_file: String,
}

fn parse_policy_manifest(text: &str, location: &Path) -> Result<PolicyManifest, String> {
    let manifest: PolicyManifest = toml::from_str(text).map_err(|error| {
        format!(
            "cannot parse policy manifest {}: {error}",
            location.display()
        )
    })?;
    let mut hooks = HashSet::new();
    if let Some(startup) = &manifest.startup {
        validate_rhai_source_path(&startup.source, location, "startup")?;
        if startup.entry.trim().is_empty() {
            return Err(format!(
                "policy manifest {} has an empty startup entry",
                location.display()
            ));
        }
    }
    for spec in &manifest.policies {
        if spec.hook.trim().is_empty() {
            return Err(format!(
                "policy manifest {} contains an empty hook id",
                location.display()
            ));
        }
        if spec.entry.trim().is_empty() {
            return Err(format!(
                "policy manifest {} has an empty entry for hook '{}'",
                location.display(),
                spec.hook
            ));
        }
        validate_rhai_source_path(&spec.source, location, "policy")?;
        if !hooks.insert(&spec.hook) {
            return Err(format!(
                "policy manifest {} declares hook '{}' more than once",
                location.display(),
                spec.hook
            ));
        }
    }
    Ok(manifest)
}

fn validate_rhai_source_path(source: &str, location: &Path, kind: &str) -> Result<(), String> {
    if !is_relative_asset_path(Path::new(source))
        || Path::new(source)
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("rhai")
    {
        return Err(format!(
            "policy manifest {} has unsafe or non-Rhai {kind} source '{source}'",
            location.display()
        ));
    }
    Ok(())
}

fn is_relative_asset_path(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
}

fn load_policy_sources(
    manifest: PolicyManifest,
    root: Option<&Path>,
    location: &Path,
    policy_prefix: &str,
) -> Result<Vec<LoadedPolicy>, String> {
    manifest
        .policies
        .into_iter()
        .map(|spec| {
            let source = load_policy_source(&spec.source, root, location)?;
            Ok(LoadedPolicy {
                policy_file: format!("{policy_prefix}/{}", spec.source),
                spec,
                source,
            })
        })
        .collect()
}

fn load_policy_source(
    source: &str,
    root: Option<&Path>,
    location: &Path,
) -> Result<String, String> {
    match root {
        Some(root) => {
            let path = root.join(source);
            let bytes = lunco_storage::read_file_sync(&path).map_err(|error| {
                format!("cannot read policy source {}: {error}", path.display())
            })?;
            String::from_utf8(bytes)
                .map_err(|error| format!("policy source {} is not UTF-8: {error}", path.display()))
        }
        None => POLICY
            .get_file(source)
            .and_then(|file| file.contents_utf8())
            .map(str::to_owned)
            .ok_or_else(|| {
                format!(
                    "embedded policy source '{source}' listed by {} is missing",
                    location.display()
                )
            }),
    }
}

fn load_policy_bundle(
    manifest: PolicyManifest,
    root: Option<&Path>,
    location: &Path,
    policy_prefix: &str,
) -> Result<LoadedPolicyBundle, String> {
    let startup = manifest.startup.clone();
    let startup = startup
        .map(|startup| -> Result<LoadedStartup, String> {
            let source = load_policy_source(&startup.source, root, location)?;
            Ok(LoadedStartup {
                policy_file: format!("{policy_prefix}/{}", startup.source),
                spec: startup,
                source,
            })
        })
        .transpose()?;
    let policies = load_policy_sources(manifest, root, location, policy_prefix)?;
    Ok(LoadedPolicyBundle { startup, policies })
}

fn require_application_startup(
    bundle: LoadedPolicyBundle,
    location: &Path,
) -> Result<LoadedPolicyBundle, String> {
    if bundle.startup.is_none() {
        return Err(format!(
            "application policy manifest {} does not define a startup entry",
            location.display()
        ));
    }
    Ok(bundle)
}

fn manifest_from_file(path: &Path) -> Result<Option<PolicyManifest>, String> {
    match lunco_storage::read_file_sync(path) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes).map_err(|error| {
                format!("policy manifest {} is not UTF-8: {error}", path.display())
            })?;
            parse_policy_manifest(&text, path).map(Some)
        }
        Err(lunco_storage::StorageError::NotFound) => Ok(None),
        Err(error) => Err(format!(
            "cannot read policy manifest {}: {error}",
            path.display()
        )),
    }
}

/// Load the application policy set and its authored startup function at sim startup.
///
/// A native editable `assets/scripting/policy/` directory must contain its
/// `index.toml`; otherwise the loader uses the packaged manifest and embedded
/// sources. A present but malformed or incomplete editable set is an explicit
/// startup diagnostic, never silently replaced by an older generation.
pub fn active_policy_set() -> Result<Vec<LoadedPolicy>, String> {
    Ok(active_policy_bundle()?.policies)
}

/// Load the application startup function and its manifest-selected policies.
pub fn active_policy_bundle() -> Result<LoadedPolicyBundle, String> {
    #[cfg(not(target_arch = "wasm32"))]
    let manifest_path = crate::assets_dir_abs().join("scripting/policy/index.toml");
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(manifest) = manifest_from_file(&manifest_path)? {
        return require_application_startup(
            load_policy_bundle(
                manifest,
                Some(manifest_path.parent().expect("manifest has a parent")),
                &manifest_path,
                "assets/scripting/policy",
            )?,
            &manifest_path,
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    if matches!(
        lunco_storage::entry_kind_file_sync(manifest_path.parent().expect("manifest has a parent")),
        Ok(lunco_storage::StorageEntryKind::Directory)
    ) {
        return Err(format!(
            "editable policy directory {} is present but index.toml is missing",
            manifest_path.parent().unwrap().display()
        ));
    }
    require_application_startup(
        load_policy_bundle(
            parse_policy_manifest(POLICY_MANIFEST, Path::new("embedded policy index.toml"))?,
            None,
            Path::new("embedded policy index.toml"),
            "assets/scripting/policy",
        )?,
        Path::new("embedded policy index.toml"),
    )
}

/// Load the active Twin's optional policy set and its separate startup function.
///
/// Twin policy sources live under `<twin>/policies/` and are selected by its
/// authored `index.toml`. No application defaults are substituted here: a
/// Twin with no policy directory simply contributes no overrides. Application
/// code decides whether and how to layer the returned Twin bundle over the
/// application bundle.
pub fn twin_policy_set(root: &Path) -> Result<Option<LoadedPolicyBundle>, String> {
    let policy_root = root.join("policies");
    let manifest_path = policy_root.join("index.toml");
    let Some(manifest) = manifest_from_file(&manifest_path)? else {
        return match lunco_storage::entry_kind_file_sync(&policy_root) {
            Ok(lunco_storage::StorageEntryKind::Directory) => Err(format!(
                "Twin policy directory {} is present but index.toml is missing",
                policy_root.display()
            )),
            Ok(_) => Err(format!(
                "Twin policy path {} is not a directory",
                policy_root.display()
            )),
            Err(lunco_storage::StorageError::NotFound) => Ok(None),
            Err(error) => Err(format!(
                "cannot inspect Twin policy directory {}: {error}",
                policy_root.display()
            )),
        };
    };
    let bundle = load_policy_bundle(manifest, Some(&policy_root), &manifest_path, "policies")?;
    if bundle.startup.is_none() && !bundle.policies.is_empty() {
        return Err(format!(
            "Twin policy manifest {} defines policies but no startup entry",
            manifest_path.display()
        ));
    }
    Ok(Some(bundle))
}

/// One embedded policy's source by file stem. Prefer [`active_policy_set`] for
/// runtime loading because it also resolves the authored manifest.
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
        let active = active_policy_set().expect("active policy source set");
        assert!(!active.is_empty(), "active policy manifest empty");
        assert!(active.iter().all(|policy| policy.source.contains("fn ")));
        let mut hooks = active
            .iter()
            .map(|policy| policy.spec.hook.as_str())
            .collect::<Vec<_>>();
        hooks.sort_unstable();
        assert!(hooks.windows(2).all(|pair| pair[0] != pair[1]));
        // Known built-ins are present (guards a broken move / path).
        let tool_names: Vec<_> = tool_libraries().into_iter().map(|(n, _)| n).collect();
        for t in [
            "assembly_edit",
            "assembly_ui",
            "formation",
            "survey",
            "debug_viz",
        ] {
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
        lunco_storage::ensure_directory_sync(&empty).expect("empty Rhai directory");
        let error = disk_rhai_files(&empty).expect_err("empty editable source is invalid");
        assert!(error.contains("contains no .rhai files"), "{error}");

        lunco_storage::write_file_sync(&empty.join("policy.rhai"), b"fn policy() { true }")
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
