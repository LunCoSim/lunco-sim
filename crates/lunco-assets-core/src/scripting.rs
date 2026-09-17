//! rhai scripting assets — the prelude, built-in tool libraries, and example
//! scenarios authored under `assets/scripting/`.
//!
//! Why this lives HERE: `lunco-assets-core` owns shared asset interaction. Every
//! set is read from the runtime asset tree through the storage boundary; edit a
//! helper or policy, restart, and no Rust rebuild is required. A missing or
//! unreadable directory is reported to the caller instead of being replaced by
//! a compiled snapshot.
//!
//! Rhai sources are authored files in the runtime asset tree. Their role is
//! selected by the application policy layer, so this crate does not encode a
//! prelude or tool directory layout.

// Sources are loaded from the runtime asset tree. The Bevy-facing asset
// loaders handle wasm; these synchronous helpers are for native discovery.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use crate::lunco_source::ASSETS_DIR_NAME;

/// Marker that distinguishes a scripting policy manifest from other authored
/// TOML files in the runtime asset library.
pub const POLICY_MANIFEST_KIND: &str = "lunco.policy.v1";

// The runtime asset tree is the only source of these authored files. The
// directories are discovered by `AssetManifest`/Bevy on web and by the storage
// backend in the synchronous native helpers below.

fn relative_asset_path(path: &Path, root: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map_err(|error| {
            format!(
                "Rhai asset {} is outside runtime asset root {}: {error}",
                path.display(),
                root.display()
            )
        })
        .map(|relative| {
            relative
                .components()
                .filter_map(|component| component.as_os_str().to_str())
                .collect::<Vec<_>>()
                .join("/")
        })
}

fn collect_rhai_sources(
    root: &Path,
    current: &Path,
    files: &mut Vec<(String, String)>,
) -> Result<(), String> {
    for path in lunco_storage::read_directory_sync(current).map_err(|error| {
        format!("cannot read Rhai asset directory {}: {error}", current.display())
    })? {
        if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
            name.starts_with('.') || name == "target"
        }) {
            continue;
        }
        match lunco_storage::entry_kind_file_sync(&path) {
            Ok(lunco_storage::StorageEntryKind::Directory) => {
                collect_rhai_sources(root, &path, files)?;
            }
            Ok(lunco_storage::StorageEntryKind::File)
                if path.extension().and_then(|x| x.to_str()) == Some("rhai") =>
            {
                let id = relative_asset_path(&path, root)?;
                let source = lunco_storage::read_text_file_sync(&path).map_err(|error| {
                    format!("cannot read Rhai asset {}: {error}", path.display())
                })?;
                files.push((id, source));
            }
            Ok(lunco_storage::StorageEntryKind::File) => {}
            Err(error) => {
                return Err(format!(
                    "cannot inspect Rhai asset {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

/// Every authored Rhai source as `(asset-relative-id, source)`, sorted by id.
///
/// The role of a source is not inferred here. The scripting runtime asks its
/// authored classification hook whether a source is prelude, a tool library,
/// or unrelated scenario content. This keeps the asset layer reusable and
/// prevents a directory convention from becoming a second policy engine.
pub fn rhai_sources() -> Result<Vec<(String, String)>, String> {
    let root = crate::assets_dir_abs();
    let mut files = Vec::new();
    collect_rhai_sources(&root, &root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    if files.is_empty() {
        return Err(format!(
            "runtime asset root {} contains no .rhai sources",
            root.display()
        ));
    }
    Ok(files)
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
    /// Authored discriminator used by runtime discovery.
    pub kind: String,
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
    if manifest.kind != POLICY_MANIFEST_KIND {
        return Err(format!(
            "policy manifest {} has unsupported kind '{}'",
            location.display(),
            manifest.kind
        ));
    }
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
                policy_file: qualified_asset_path(policy_prefix, &spec.source),
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
        None => Err(format!(
            "policy source '{source}' listed by {} needs the external asset loader",
            location.display()
        )),
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
                policy_file: qualified_asset_path(policy_prefix, &startup.source),
                spec: startup,
                source,
            })
        })
        .transpose()?;
    let policies = load_policy_sources(manifest, root, location, policy_prefix)?;
    Ok(LoadedPolicyBundle { startup, policies })
}

fn qualified_asset_path(prefix: &str, source: &str) -> String {
    if prefix.is_empty() {
        source.to_owned()
    } else {
        format!("{prefix}/{source}")
    }
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

fn collect_toml_sources(
    current: &Path,
    files: &mut Vec<(PathBuf, String)>,
) -> Result<(), String> {
    for path in lunco_storage::read_directory_sync(current).map_err(|error| {
        format!("cannot read asset directory {}: {error}", current.display())
    })? {
        if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
            name.starts_with('.') || name == "target"
        }) {
            continue;
        }
        match lunco_storage::entry_kind_file_sync(&path) {
            Ok(lunco_storage::StorageEntryKind::Directory) => {
                collect_toml_sources(&path, files)?;
            }
            Ok(lunco_storage::StorageEntryKind::File)
                if path.extension().and_then(|x| x.to_str()) == Some("toml") =>
            {
                let source = lunco_storage::read_text_file_sync(&path).map_err(|error| {
                    format!("cannot read asset {}: {error}", path.display())
                })?;
                files.push((path, source));
            }
            Ok(lunco_storage::StorageEntryKind::File) => {}
            Err(error) => {
                return Err(format!(
                    "cannot inspect asset {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn relative_asset_prefix(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| format!("asset {} is outside {}: {error}", path.display(), root.display()))?;
    let parent = relative.parent().unwrap_or_else(|| Path::new("."));
    let parent = parent
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/");
    Ok(parent)
}

fn asset_prefix(root: &Path, path: &Path) -> Result<String, String> {
    let parent = relative_asset_prefix(root, path)?;
    Ok(if parent.is_empty() {
        ASSETS_DIR_NAME.to_owned()
    } else {
        format!("{ASSETS_DIR_NAME}/{parent}")
    })
}

/// Load the application policy set and its authored startup function at sim startup.
///
/// The native synchronous path discovers the uniquely marked application policy
/// manifest from the runtime asset tree and reads its sources through storage.
/// Web startup uses the asset pipeline and must not call this synchronous helper.
pub fn active_policy_set() -> Result<Vec<LoadedPolicy>, String> {
    Ok(active_policy_bundle()?.policies)
}

/// Load the application startup function and its manifest-selected policies.
pub fn active_policy_bundle() -> Result<LoadedPolicyBundle, String> {
    #[cfg(not(target_arch = "wasm32"))]
    let assets_root = crate::assets_dir_abs();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut files = Vec::new();
        collect_toml_sources(&assets_root, &mut files)?;
        let mut candidates = Vec::new();
        for (path, text) in files {
            let is_policy = toml::from_str::<toml::Value>(&text)
                .ok()
                .and_then(|value| value.get("kind").and_then(toml::Value::as_str).map(str::to_owned))
                .is_some_and(|kind| kind == POLICY_MANIFEST_KIND);
            if is_policy {
                candidates.push((path, text));
            }
        }
        if candidates.len() != 1 {
            return Err(match candidates.len() {
                0 => format!(
                    "runtime asset tree {} contains no application policy manifest marked kind '{}'",
                    assets_root.display(),
                    POLICY_MANIFEST_KIND
                ),
                count => format!(
                    "runtime asset tree {} contains {count} application policy manifests; exactly one is required",
                    assets_root.display()
                ),
            });
        }
        let (manifest_path, text) = candidates
            .pop()
            .ok_or_else(|| "application policy manifest candidate disappeared".to_owned())?;
        let manifest = parse_policy_manifest(&text, &manifest_path)?;
        let policy_root = manifest_path
            .parent()
            .ok_or_else(|| format!("policy manifest {} has no parent", manifest_path.display()))?;
        let prefix = asset_prefix(&assets_root, &manifest_path)?;
        return require_application_startup(
            load_policy_bundle(manifest, Some(policy_root), &manifest_path, &prefix)?,
            &manifest_path,
        );
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err("application policy assets are not available through the synchronous loader on this platform".into())
    }
}

/// Load the active Twin's optional policy set and its separate startup function.
///
/// Twin policy sources are selected by the uniquely marked authored policy
/// manifest anywhere under the Twin root. No application defaults are
/// substituted here: a Twin with no policy manifest simply contributes no
/// overrides. Application code decides whether and how to layer the returned
/// Twin bundle over the application bundle.
pub fn twin_policy_set(root: &Path) -> Result<Option<LoadedPolicyBundle>, String> {
    let mut files = Vec::new();
    collect_toml_sources(root, &mut files)?;
    let mut candidates = files
        .into_iter()
        .filter_map(|(path, text)| {
            let is_policy = toml::from_str::<toml::Value>(&text)
                .ok()
                .and_then(|value| {
                    value
                        .get("kind")
                        .and_then(toml::Value::as_str)
                        .map(str::to_owned)
                })
                .is_some_and(|kind| kind == POLICY_MANIFEST_KIND);
            is_policy.then_some((path, text))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(None);
    }
    if candidates.len() != 1 {
        return Err(format!(
            "Twin root {} contains {} policy manifests marked kind '{}'; exactly one is required",
            root.display(),
            candidates.len(),
            POLICY_MANIFEST_KIND
        ));
    }
    let (manifest_path, text) = candidates
        .pop()
        .ok_or_else(|| "Twin policy manifest candidate disappeared".to_owned())?;
    let manifest = parse_policy_manifest(&text, &manifest_path)?;
    let policy_root = manifest_path
        .parent()
        .ok_or_else(|| format!("policy manifest {} has no parent", manifest_path.display()))?;
    let prefix = relative_asset_prefix(root, &manifest_path)?;
    let bundle = load_policy_bundle(manifest, Some(policy_root), &manifest_path, &prefix)?;
    if bundle.startup.is_none() && !bundle.policies.is_empty() {
        return Err(format!(
            "Twin policy manifest {} defines policies but no startup entry",
            manifest_path.display()
        ));
    }
    Ok(Some(bundle))
}
