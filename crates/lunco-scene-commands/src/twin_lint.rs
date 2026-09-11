//! Twin-wide namespace facts for the explicit lint/preflight paths.
//!
//! A Twin is not one flat basename directory. Modelica classes and Rhai tool
//! libraries have resolver scopes; USD, shader, and other asset references are
//! addressed by their complete source path. This module records those
//! boundaries and only groups names in a resolver that can actually be
//! ambiguous. Rhai policy decides the severity and final finding text.

use lunco_hooks::HookValue as H;
use lunco_usd_bevy::UsdRead;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

const MODELICA_NAMESPACE: &str = "modelica-class";
const USD_DEFAULT_NAMESPACE: &str = "usd-default-prim";
const USD_NAMESPACE: &str = "usd-prim";
const SHADER_NAMESPACE: &str = "shader-module";
const ASSET_NAMESPACE: &str = "asset-stem";
const RHAI_NAMESPACE: &str = "rhai-tool-library";
const RHAI_SCOPE: &str = "active Twin tool modules";

/// One name that participates in a real resolver namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NamespaceEntry {
    /// Resolver family, such as `modelica-class` or `rhai-tool-library`.
    pub namespace: String,
    /// Name used by that resolver.
    pub name: String,
    /// Owning domain or runtime layer.
    pub owner: String,
    /// Twin-relative source path or an explicit runtime source description.
    pub source: String,
    /// Resolution scope in which the name is looked up.
    pub scope: String,
    /// Human-readable rule used to resolve the name.
    pub resolution: String,
}

/// A deterministic group of entries that share one ambiguous resolver key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NamespaceCollision {
    /// Resolver family.
    pub namespace: String,
    /// Colliding name.
    pub name: String,
    /// Resolver scope.
    pub scope: String,
    /// Rule the author must follow to remove the ambiguity.
    pub resolution: String,
    /// Every owner, source, domain and scope participating in the collision.
    pub entries: Vec<NamespaceEntry>,
}

/// Read-only snapshot used by both live `RunLint { scope: "twin" }` and
/// `ValidateTwin`. The snapshot is also the serializable preflight payload.
#[derive(Debug, Clone, Serialize)]
pub struct TwinNamespaceSnapshot {
    /// Twin display name.
    pub twin: String,
    /// Absolute root used for native source reads.
    pub root: String,
    /// Resolver entries whose names can be ambiguous.
    pub entries: Vec<NamespaceEntry>,
    /// Deterministic collisions in those resolver scopes.
    pub collisions: Vec<NamespaceCollision>,
    /// Source reads that could not be completed. These stay visible to policy;
    /// the inspector never treats an unreadable source as an empty namespace.
    pub read_errors: Vec<String>,
}

/// Normalize the only supported Twin namespace severity policies.
pub fn policy_name(policy: &str) -> Result<&'static str, String> {
    match policy.trim() {
        "" | "warn" => Ok("warn"),
        "error" => Ok("error"),
        invalid => Err(format!(
            "unknown Twin namespace policy `{invalid}`; use `warn` or `error`"
        )),
    }
}

/// Inspect one indexed Twin without mutating its manifest, files, or runtime.
pub fn inspect_twin(twin: &lunco_workspace::Twin) -> TwinNamespaceSnapshot {
    let mut entries = Vec::new();
    let mut read_errors = Vec::new();
    let twin_name = twin_name(twin);

    let modelica_roots = modelica_roots(twin);
    for file in twin.files() {
        let rel = &file.relative_path;
        if !has_extension(rel, "mo") {
            continue;
        }
        let source = match read_twin_text(twin, rel) {
            Ok(source) => source,
            Err(error) => {
                read_errors.push(format!("{}: {error}", slashed(rel)));
                continue;
            }
        };
        let scope = modelica_scope(rel, &modelica_roots);
        for name in lunco_modelica_ast::ast_extract::declared_class_names(&source, &slashed(rel)) {
            entries.push(NamespaceEntry {
                namespace: MODELICA_NAMESPACE.to_string(),
                name,
                owner: "Modelica source root".to_string(),
                source: slashed(rel),
                scope: scope.clone(),
                resolution: "qualified class lookup within one Twin Modelica source root"
                    .to_string(),
            });
        }
    }

    for file in twin.files() {
        let rel = &file.relative_path;
        add_asset_entry(rel, &mut entries);
        if has_extension(rel, "wgsl") {
            entries.push(NamespaceEntry {
                namespace: SHADER_NAMESPACE.to_string(),
                name: file_stem(rel),
                owner: "Twin shader module".to_string(),
                source: slashed(rel),
                scope: format!(
                    "shader directory `{}`",
                    slashed(rel.parent().unwrap_or_else(|| Path::new("")))
                ),
                resolution: "shader module lookup by stem within one Twin shader directory"
                    .to_string(),
            });
        }
        if has_extension(rel, "usda") || has_extension(rel, "usd") {
            inspect_usd_file(twin, rel, &mut entries, &mut read_errors);
        }
    }

    // The source files are the durable Twin owner. The built-in names and the
    // live registry are both included so a Twin tool that shadows an engine
    // module is reported even though the process registry retains one winner.
    let mut known_tools = HashSet::new();
    match lunco_assets::scripting::active_tool_libraries() {
        Ok(tools) => {
            for (name, _) in tools {
                known_tools.insert(name.clone());
                entries.push(tool_entry(
                    name.clone(),
                    "engine tool library",
                    format!("assets/scripting/tools/{}.rhai", name),
                ));
            }
        }
        Err(error) => read_errors.push(format!("assets/scripting/tools: {error}")),
    }
    known_tools.insert("mathx".to_string());
    if lunco_tools::get("mathx").is_some() {
        entries.push(tool_entry(
            "mathx".to_string(),
            "engine native tool",
            "runtime native registration".to_string(),
        ));
    }

    for file in twin.files() {
        let rel = &file.relative_path;
        if rel.parent() != Some(Path::new("tools")) || !has_extension(rel, "rhai") {
            continue;
        }
        let Some(name) = rel.file_stem().and_then(|stem| stem.to_str()) else {
            read_errors.push(format!("{}: filename is not valid UTF-8", slashed(rel)));
            continue;
        };
        entries.push(tool_entry(
            name.to_string(),
            format!("Twin `{twin_name}` tool library"),
            slashed(rel),
        ));
    }

    for tool in lunco_tools::all() {
        if known_tools.contains(tool.name())
            || entries
                .iter()
                .any(|entry| entry.namespace == RHAI_NAMESPACE && entry.name == tool.name())
        {
            continue;
        }
        let source = if tool.source().is_some() {
            "runtime source registration"
        } else {
            "runtime native registration"
        };
        entries.push(tool_entry(
            tool.name().to_string(),
            format!("runtime {} tool", tool.backend()),
            source.to_string(),
        ));
    }

    entries.sort_by(|a, b| {
        (&a.namespace, &a.scope, &a.name, &a.source).cmp(&(
            &b.namespace,
            &b.scope,
            &b.name,
            &b.source,
        ))
    });
    let collisions = collisions(&entries);

    TwinNamespaceSnapshot {
        twin: twin_name,
        root: twin.root.to_string_lossy().into_owned(),
        entries,
        collisions,
        read_errors,
    }
}

/// Convert the typed snapshot to the language-neutral facts consumed by the
/// authored `lint.twin` policy.
pub fn facts(snapshot: &TwinNamespaceSnapshot, policy: &str) -> H {
    let entry_facts = snapshot.entries.iter().map(entry_fact).collect::<Vec<_>>();
    let collision_facts = snapshot
        .collisions
        .iter()
        .map(|collision| {
            H::map([
                ("namespace", H::str(collision.namespace.clone())),
                ("name", H::str(collision.name.clone())),
                ("scope", H::str(collision.scope.clone())),
                ("resolution", H::str(collision.resolution.clone())),
                (
                    "entries",
                    H::Array(collision.entries.iter().map(entry_fact).collect()),
                ),
            ])
        })
        .collect::<Vec<_>>();
    H::map([
        ("twin", H::str(snapshot.twin.clone())),
        ("root", H::str(snapshot.root.clone())),
        ("policy", H::str(policy.to_string())),
        ("entries", H::Array(entry_facts)),
        ("collisions", H::Array(collision_facts)),
        (
            "read_errors",
            H::Array(snapshot.read_errors.iter().cloned().map(H::str).collect()),
        ),
    ])
}

fn entry_fact(entry: &NamespaceEntry) -> H {
    H::map([
        ("namespace", H::str(entry.namespace.clone())),
        ("name", H::str(entry.name.clone())),
        ("owner", H::str(entry.owner.clone())),
        ("source", H::str(entry.source.clone())),
        ("scope", H::str(entry.scope.clone())),
        ("resolution", H::str(entry.resolution.clone())),
    ])
}

fn tool_entry(name: String, owner: impl Into<String>, source: String) -> NamespaceEntry {
    NamespaceEntry {
        namespace: RHAI_NAMESPACE.to_string(),
        name,
        owner: owner.into(),
        source,
        scope: RHAI_SCOPE.to_string(),
        resolution: "one `name::function(...)` module in the active tool registry".to_string(),
    }
}

fn add_asset_entry(rel: &Path, entries: &mut Vec<NamespaceEntry>) {
    let Some(stem) = rel.file_stem().and_then(|stem| stem.to_str()) else {
        return;
    };
    let parent = rel.parent().unwrap_or_else(|| Path::new(""));
    let extension = rel
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    entries.push(NamespaceEntry {
        namespace: ASSET_NAMESPACE.to_string(),
        name: stem.to_string(),
        owner: "Twin asset file".to_string(),
        source: slashed(rel),
        scope: format!(
            "asset directory `{}` with extension `.{extension}`",
            slashed(parent)
        ),
        resolution: "relative asset lookup within one Twin directory, with the extension selecting the file format".to_string(),
    });
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

fn inspect_usd_file(
    twin: &lunco_workspace::Twin,
    rel: &Path,
    entries: &mut Vec<NamespaceEntry>,
    read_errors: &mut Vec<String>,
) {
    let path = twin.root.join(rel);
    let engine_root = crate::validate::engine_assets_root();
    let stage = match lunco_usd_compose::compose_file_to_stage_with_roots(
        &path,
        Some(engine_root.as_path()),
        Some(twin.root.as_path()),
    ) {
        Ok(stage) => stage,
        Err(error) => {
            read_errors.push(format!("{}: {error}", slashed(rel)));
            return;
        }
    };
    let canonical = lunco_usd_bevy::CanonicalStage::from_stage(stage, slashed(rel));
    let view = canonical.view();
    let scope = format!("composed USD stage `{}`", slashed(rel));
    if let Some(default_prim) = view.default_prim() {
        entries.push(NamespaceEntry {
            namespace: USD_DEFAULT_NAMESPACE.to_string(),
            name: default_prim,
            owner: "USD composed defaultPrim".to_string(),
            source: slashed(rel),
            scope: scope.clone(),
            resolution: "one defaultPrim selects the root identity mounted from this USD stage"
                .to_string(),
        });
    }
    for prim in view.prim_paths() {
        let Some(name) = prim
            .as_str()
            .strip_prefix('/')
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let type_name = view.prim_type_name(&prim);
        let owner = match type_name.as_deref() {
            Some("Shader") => "USD composed Shader prim",
            _ => "USD composed prim",
        };
        entries.push(NamespaceEntry {
            namespace: USD_NAMESPACE.to_string(),
            name: name.to_string(),
            owner: owner.to_string(),
            source: slashed(rel),
            scope: scope.clone(),
            resolution: "full USD prim path within one composed stage".to_string(),
        });
    }
}

fn collisions(entries: &[NamespaceEntry]) -> Vec<NamespaceCollision> {
    let mut groups: BTreeMap<(String, String, String), Vec<NamespaceEntry>> = BTreeMap::new();
    for entry in entries {
        groups
            .entry((
                entry.namespace.clone(),
                entry.scope.clone(),
                entry.name.clone(),
            ))
            .or_default()
            .push(entry.clone());
    }
    groups
        .into_iter()
        .filter_map(|((namespace, scope, name), mut entries)| {
            (entries.len() > 1).then(|| {
                entries.sort_by(|a, b| (&a.owner, &a.source).cmp(&(&b.owner, &b.source)));
                let resolution = entries[0].resolution.clone();
                NamespaceCollision {
                    namespace,
                    name,
                    scope,
                    resolution,
                    entries,
                }
            })
        })
        .collect()
}

fn twin_name(twin: &lunco_workspace::Twin) -> String {
    twin.manifest
        .as_ref()
        .map(|manifest| manifest.name.clone())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            twin.root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Twin".to_string())
}

fn modelica_roots(twin: &lunco_workspace::Twin) -> Vec<PathBuf> {
    let declared = twin
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.modelica.as_ref())
        .map(|modelica| modelica.paths.clone());
    let paths = declared
        .unwrap_or_else(|| lunco_modelica::source_roots::discover_twin_modelica_paths(twin));
    paths
        .into_iter()
        .filter(|path| lunco_twin::is_safe_relative_path(path))
        .map(|path| lunco_assets::asset_path::normalize(&path))
        .collect()
}

fn modelica_scope(rel: &Path, roots: &[PathBuf]) -> String {
    let parent = rel.parent().unwrap_or_else(|| Path::new(""));
    let root = roots
        .iter()
        .filter(|root| root.as_os_str().is_empty() || parent.starts_with(root))
        .max_by_key(|root| root.components().count())
        .map(|root| slashed(root))
        .unwrap_or_else(|| slashed(parent));
    format!("modelica source root `{root}`")
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}

fn slashed(path: &Path) -> String {
    lunco_assets::asset_path::slashed(path)
}

fn read_twin_text(twin: &lunco_workspace::Twin, rel: &Path) -> Result<String, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let id = format!("twin://namespace-lint/{}", slashed(rel));
        let bytes =
            lunco_assets::read_asset_bytes_with_twin_root(&id, None, Some(twin.root.as_path()))
                .map_err(|error| error.to_string())?;
        String::from_utf8(bytes).map_err(|error| error.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (twin, rel);
        Err("Twin source inspection is unavailable without a native Twin file reader".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(namespace: &str, scope: &str, name: &str, source: &str) -> NamespaceEntry {
        NamespaceEntry {
            namespace: namespace.to_string(),
            name: name.to_string(),
            owner: "test".to_string(),
            source: source.to_string(),
            scope: scope.to_string(),
            resolution: "test resolver".to_string(),
        }
    }

    #[test]
    fn collision_index_is_deterministic_and_scope_aware() {
        let entries = vec![
            entry("modelica-class", "root-a", "Drive", "a.mo"),
            entry("modelica-class", "root-b", "Drive", "b.mo"),
            entry("modelica-class", "root-a", "Drive", "c.mo"),
            entry("rhai-tool-library", "global", "Drive", "drive.rhai"),
        ];
        let found = collisions(&entries);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].namespace, "modelica-class");
        assert_eq!(found[0].scope, "root-a");
        assert_eq!(found[0].entries.len(), 2);
        assert_eq!(found[0].entries[0].source, "a.mo");
    }

    #[test]
    fn indexed_twin_reports_real_same_root_collision_without_cross_root_false_positive() {
        let temp = tempfile::tempdir().expect("temporary Twin root");
        std::fs::create_dir_all(temp.path().join("other")).expect("nested source root");
        std::fs::write(temp.path().join("a.mo"), "model Drive end Drive;")
            .expect("first Modelica source");
        std::fs::write(temp.path().join("b.mo"), "model Drive end Drive;")
            .expect("second Modelica source");
        std::fs::write(temp.path().join("other/Drive.mo"), "model Drive end Drive;")
            .expect("independent Modelica source root");

        let mode = lunco_twin::TwinMode::open(temp.path()).expect("open Twin folder");
        let twin = match mode {
            lunco_twin::TwinMode::Folder(twin) | lunco_twin::TwinMode::Twin(twin) => twin,
            lunco_twin::TwinMode::Orphan(_) => panic!("temporary path is a folder"),
        };
        let snapshot = inspect_twin(&twin);
        let collisions = snapshot
            .collisions
            .iter()
            .filter(|collision| {
                collision.namespace == MODELICA_NAMESPACE && collision.name == "Drive"
            })
            .collect::<Vec<_>>();
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].entries.len(), 2);
        assert_eq!(collisions[0].scope, "modelica source root ``");
        assert!(collisions[0]
            .entries
            .iter()
            .all(|entry| entry.source == "a.mo" || entry.source == "b.mo"));
        assert!(snapshot
            .entries
            .iter()
            .any(|entry| { entry.namespace == ASSET_NAMESPACE && entry.source == "a.mo" }));
    }
}
