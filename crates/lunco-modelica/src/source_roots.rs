//! Per-library / per-package source root registry.
//!
//! Inventory of every named source root the workbench knows how to
//! load into the rumoca compile session. A "source root" is any
//! qualified-path-root segment that compiles can depend on:
//!
//! - **System libraries**: MSL (`Modelica`), third-party libraries
//!   discovered in the `lunco-assets` cache (`ThermofluidStream`,
//!   etc.). Loaded from disk via
//!   `session.load_source_root_tolerant(...)`.
//! - **Bundled examples**: `.mo` files compiled into the binary
//!   (`AnnotatedRocketStage`, `Balloon`, etc.). Loaded via
//!   [`crate::models::get_model`].
//! - **Workspace files**: user-authored `.mo` files in the active
//!   workspace tree.
//!
//! ## Design intent
//!
//! Generalises the MSL-only load path
//! ([`crate::msl_remote::MslRemotePlugin`]) so that every source the
//! compiler needs goes through one registry with one state machine.
//! Adding a fourth system library, a new bundled example, or a
//! workspace folder becomes a data change, not new plumbing.

use bevy::prelude::*;
use rumoca_compile::parsing::ast::{ClassDef, StoredDefinition};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use web_time::Instant;

/// Per-source-root state. Mirrors the `MslLoadState` shape, but
/// keyed at the entry level instead of being a singleton.
#[derive(Debug, Clone)]
pub enum LoadState {
    /// Discovered, but no load attempt yet. The default for every
    /// registered root at plugin start.
    NotLoaded,
    /// A background load is in flight. `progress` is `0.0..=1.0`
    /// when the loader reports it; phases without a known total
    /// keep it at 0.0.
    Loading { progress: f32, started: Instant },
    /// Source has been installed into the rumoca session. Compiles
    /// that depend on this root can dispatch.
    Ready,
    /// Last load attempt failed. Compile gate surfaces the message
    /// to the console / status bus and lets the dependent compile
    /// fail cleanly rather than retry indefinitely.
    Failed(String),
}

/// How to actually fetch + install the source for one root.
///
/// The source-root loader matches on this enum to pick the correct strategy.
#[derive(Debug, Clone)]
pub enum SourceRootKind {
    /// On-disk Modelica library (MSL or third-party). Loaded via
    /// `session.load_source_root_tolerant`.
    SystemLibrary {
        /// `lunco-assets` cache subdirectory the library was unpacked
        /// to. `"msl"` for MSL, `"thermofluidstream"` for
        /// ThermofluidStream, etc.
        cache_subdir: String,
        /// Absolute path to the package root directory containing
        /// `package.mo`.
        root_dir: PathBuf,
    },
    /// Bundled example shipped inside the binary. Source bytes come
    /// from [`crate::models::get_model`]; install path is the same
    /// document-registry pipeline used when the user opens a bundled
    /// model from the package browser, but driven by the compile
    /// gate instead of a UI gesture.
    Bundled {
        /// `.mo` filename inside the embedded `models/` directory
        /// (e.g. `"AnnotatedRocketStage.mo"`).
        filename: String,
    },
    /// Structured Modelica package embedded below `assets/models/<root>`.
    /// The complete package tree is loaded from the asset owner, so package
    /// members resolve by their authored `within` names like any package on
    /// a normal Modelica search path.
    BundledPackage { root: String },
    /// User `.mo` file in the active workspace. Loaded by reading
    /// the file from disk and installing the resulting document.
    /// Populated when a workspace document contributes a source root.
    WorkspaceFile {
        /// Absolute path on disk.
        path: PathBuf,
    },
    /// Source already synchronized from an untitled editor document.
    ///
    /// This is deliberately distinct from [`SourceRootKind::Bundled`]: an
    /// untitled document has no embedded filename and must never be sent
    /// through the bundled-model loader.
    SessionDocument { id: String },
}

/// One source root the workbench knows about. Keyed by the
/// qualified-path root segment (the value the dep-scanner extracts
/// from a `Modelica.Blocks.X` reference is `"Modelica"`).
#[derive(Debug, Clone)]
pub struct SourceRoot {
    /// Root segment of qualified names that resolve into this source.
    /// MSL: `"Modelica"`. ThermofluidStream: `"ThermofluidStream"`.
    /// Bundled `AnnotatedRocketStage.mo`: `"AnnotatedRocketStage"`.
    pub id: String,
    /// How to actually load this root when the gate decides to.
    pub kind: SourceRootKind,
    /// Current load state. Transitions:
    /// `NotLoaded` → `Loading` (gate kicks off bg task)
    /// `Loading` → `Ready` / `Failed` (loader completes).
    pub state: LoadState,
}

/// Process-wide registry of every named source root. Owned by the
/// `ModelicaPlugin`; populated at plugin start by inventorying:
///  - MSL via [`lunco_assets::msl_source_root_path`].
///  - Third-party libraries via
///    [`crate::package_tree::scanner::discover_third_party_libs`].
///  - Bundled examples via [`crate::models::bundled_models`].
///  - Structured packages via [`lunco_assets::models::package_roots_live`].
///
/// Loading remains demand-driven: inventory is cheap, and a root is installed
/// only when a compile or class lookup actually references it.
#[derive(Resource, Debug, Default)]
pub struct SourceRootRegistry {
    /// Map of root id → entry. The dep-scanner looks up qualified-
    /// path roots here; the gate transitions state on each entry.
    pub roots: HashMap<String, SourceRoot>,
}

impl SourceRootRegistry {
    /// Build the inventory. Runs once at plugin start.
    ///
    /// Logs a one-line summary per kind so it's easy to confirm the
    /// registry contents match what the user has installed.
    pub fn build() -> Self {
        let mut roots: HashMap<String, SourceRoot> = HashMap::new();

        // MSL — the canonical system library. If `lunco-assets` hasn't
        // unpacked it, we skip; the dep-scanner will still see
        // `Modelica.*` references and surface the missing-library
        // error via the gate.
        if let Some(msl_dir) = lunco_assets::msl_source_root_path() {
            roots.insert(
                "Modelica".to_string(),
                SourceRoot {
                    id: "Modelica".to_string(),
                    kind: SourceRootKind::SystemLibrary {
                        cache_subdir: "msl".to_string(),
                        root_dir: msl_dir,
                    },
                    state: LoadState::NotLoaded,
                },
            );
        }

        // Third-party libraries — every package with a `package.mo`
        // under a sibling of `<cache>/msl/`. Discovery already
        // implemented for the package-browser tree; we reuse it here
        // for the compile-gate registry.
        for (cache_subdir, root_name) in crate::package_tree::scanner::discover_third_party_libs() {
            let root_dir = lunco_assets::cache_dir()
                .join(&cache_subdir)
                .join(&root_name);
            roots.insert(
                root_name.clone(),
                SourceRoot {
                    id: root_name,
                    kind: SourceRootKind::SystemLibrary {
                        cache_subdir,
                        root_dir,
                    },
                    state: LoadState::NotLoaded,
                },
            );
        }

        // Bundled examples — keyed by filename stem (the convention
        // every bundled `.mo` follows: `Foo.mo` contains `package Foo`
        // or `model Foo`). The dep-scanner extracts `Foo` from a
        // `Foo.X` reference and looks it up here.
        for model in crate::models::bundled_models() {
            let Some(id) = model.filename.strip_suffix(".mo") else {
                continue;
            };
            // Don't shadow a system library with a bundled entry —
            // MSL / third-party wins. (No current bundled file
            // collides, but worth being explicit.)
            if roots.contains_key(id) {
                continue;
            }
            roots.insert(
                id.to_string(),
                SourceRoot {
                    id: id.to_string(),
                    kind: SourceRootKind::Bundled {
                        filename: model.filename.to_string(),
                    },
                    state: LoadState::NotLoaded,
                },
            );
        }

        // Structured packages — keyed by their Modelica root segment. On
        // native, prefer the live package directory so editor changes are
        // visible without rebuilding; on wasm, use the embedded package tree.
        // This is the standard root-segment search-path inventory, not a
        // library-specific registration.
        for root_name in lunco_assets::models::package_roots_live() {
            if roots.contains_key(&root_name) {
                continue;
            }
            let kind = lunco_assets::models_package_root_path(&root_name)
                .map(|root_dir| SourceRootKind::SystemLibrary {
                    cache_subdir: format!("models/{root_name}"),
                    root_dir,
                })
                .unwrap_or_else(|| SourceRootKind::BundledPackage {
                    root: root_name.clone(),
                });
            roots.insert(
                root_name.clone(),
                SourceRoot {
                    id: root_name,
                    kind,
                    state: LoadState::NotLoaded,
                },
            );
        }

        let lib_count = roots
            .values()
            .filter(|r| matches!(r.kind, SourceRootKind::SystemLibrary { .. }))
            .count();
        let bundled_count = roots
            .values()
            .filter(|r| {
                matches!(
                    r.kind,
                    SourceRootKind::Bundled { .. } | SourceRootKind::BundledPackage { .. }
                )
            })
            .count();
        bevy::log::info!(
            "[source-roots] registry built: {} system libraries, {} bundled examples \
             (all NotLoaded)",
            lib_count,
            bundled_count,
        );

        Self { roots }
    }

    /// Query: does the dep-scanner's root segment refer to a known
    /// source root? Useful for telling apart real library deps from
    /// typos / unknown packages (which should let compile fall
    /// through to rumoca's error path).
    pub fn contains(&self, id: &str) -> bool {
        self.roots.contains_key(id)
    }

    /// Insert / refresh an entry for a workspace-or-document-backed
    /// source root and mark it `Ready`. Used by the doc-opened
    /// observer to register every open doc's top-level package
    /// names as already-loaded — they're synced into the rumoca
    /// session by `engine_resource::drive_engine_sync` immediately
    /// on install, so the dep gate should treat them as Ready
    /// without a worker round-trip.
    ///
    /// Idempotent: re-registering an existing entry keeps the
    /// existing `kind` if it's a SystemLibrary (a workspace doc
    /// must not shadow MSL), otherwise overwrites with the new
    /// metadata. Always flips state to `Ready`.
    pub fn register_open_doc_root(&mut self, id: String, path: Option<PathBuf>) {
        // Don't let an opened doc shadow a system library entry —
        // MSL contents are loaded via its own kind, not as workspace
        // files.
        if let Some(existing) = self.roots.get(&id) {
            if matches!(existing.kind, SourceRootKind::SystemLibrary { .. }) {
                return;
            }
        }
        let kind = match path {
            Some(p) => SourceRootKind::WorkspaceFile { path: p },
            None => SourceRootKind::SessionDocument { id: id.clone() },
        };
        self.roots.insert(
            id.clone(),
            SourceRoot {
                id,
                kind,
                state: LoadState::Ready,
            },
        );
    }

    /// Borrow an entry's load state.
    pub fn state(&self, id: &str) -> Option<&LoadState> {
        self.roots.get(id).map(|r| &r.state)
    }
}

/// Walk an AST and extract the set of qualified-path root segments
/// that the AST references. The result is the input to the load
/// gate: each segment is looked up in [`SourceRootRegistry`] to
/// decide whether the corresponding source root needs to be loaded
/// before compile.
///
/// For example, an AST that contains
/// `Modelica.Blocks.Interfaces.RealOutput x;` and
/// `extends ThermofluidStream.Boundaries.Base;` yields
/// `{"Modelica", "ThermofluidStream"}`.
///
/// Filters out:
/// - Built-in scalar types (`Real`, `Integer`, etc.) — handled by
///   rumoca natively.
/// - Bare (non-qualified) names — those resolve locally via the
///   doc's own classes, no external load needed.
/// - The empty string (defensive).
pub fn scan_source_root_deps(ast: &StoredDefinition) -> HashSet<String> {
    let mut qualified_names: HashSet<String> = HashSet::new();
    for (_, class) in &ast.classes {
        walk_class_qualified_types(class, &mut qualified_names);
    }
    // Map qualified names to their root segments.
    qualified_names
        .into_iter()
        .filter_map(|name| name.split('.').next().map(|s| s.to_string()))
        .filter(|root| !root.is_empty() && !is_builtin_root(root))
        .collect()
}

/// Scan a source document for the Modelica search-path roots it references.
/// This is the source-side equivalent of `scan_source_root_deps` for compile
/// entry points that have source text but do not yet own an AST.
pub fn scan_source_root_deps_from_source(source: &str, uri: &str) -> HashSet<String> {
    rumoca_phase_parse::parse_to_ast(source, uri)
        .map(|ast| scan_source_root_deps(&ast))
        .unwrap_or_default()
}

/// Collect type-name references from `class`, keeping only qualified
/// (dotted) names — bare names always resolve within the current
/// doc's own classes, so they never imply an external source-root
/// load. Traversal lives in `crate::ast_extract::walk_class_type_names`
/// so this scanner and the icon warmer can't drift apart on what
/// "every referenced type" means.
fn walk_class_qualified_types(class: &ClassDef, out: &mut HashSet<String>) {
    crate::ast_extract::walk_class_type_names(class, &mut |name| {
        if name.contains('.') {
            out.insert(name.to_string());
        }
    });
    for import in &class.imports {
        use rumoca_compile::parsing::ast::Import;
        let path = match import {
            Import::Qualified { path, .. } | Import::Renamed { path, .. } => path.to_string(),
            Import::Unqualified { path, .. } => path.to_string(),
            Import::Selective { path, .. } => path.to_string(),
        };
        if path.contains('.') {
            out.insert(path);
        }
    }
}

/// Modelica built-in root segments that never need a source root
/// load. Matches the filter in
/// [`crate::icon_warmer::interesting_type`].
fn is_builtin_root(root: &str) -> bool {
    matches!(
        root,
        "Real" | "Integer" | "Boolean" | "String" | "enumeration"
    )
}

/// Ensure that the source root `id` is loaded into the rumoca
/// compile session before the next compile runs. Returns `true`
/// when the root is `Ready` (either now or after this call's
/// install). Returns `false` for unknown ids or load failures —
/// the caller logs and lets compile fall through (rumoca will
/// surface a `unresolved type reference` diagnostic).
///
/// The per-kind dispatch sends either a disk package or an in-memory package
/// to the worker. The worker owns parsing and session installation; this
/// function only changes registry state and queues the operation.
///
/// Source tag used for [`lunco_workbench::status_bus::StatusBus`]
/// progress entries during source-root loads.
pub const STATUS_BUS_SOURCE: &str = "source-roots";

/// One source root admitted from a mounted Twin.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TwinSourceRootSpec {
    id: String,
    root_dir: PathBuf,
}

/// Normalize a manifest search path for the `TwinRoots` directory resolver.
/// `TwinRoots` uses an empty relative path for the root itself, while TOML
/// authors conventionally spell that path as `"."`.
fn normalize_twin_source_path(path: &Path) -> Result<PathBuf, String> {
    if !lunco_twin::is_safe_relative_path(path) {
        return Err(format!(
            "Modelica search path `{}` must stay relative to the Twin root",
            path.display()
        ));
    }
    Ok(lunco_assets::asset_path::normalize(path))
}

/// Discover the source directories represented by a Twin's indexed `.mo` files.
/// Package directories are preferred over their nested package members, while
/// flat files remain grouped by their nearest common indexed directory. This
/// lets a Twin with no domain-specific path declaration use standard
/// `package.mo` layout without inventing a second loader.
fn discover_twin_modelica_paths(twin: &lunco_twin::Twin) -> Vec<PathBuf> {
    fn is_modelica_file(path: &Path) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mo"))
    }

    fn is_under_or_equal(path: &Path, ancestor: &Path) -> bool {
        ancestor.as_os_str().is_empty() || path == ancestor || path.starts_with(ancestor)
    }

    let mut candidates: Vec<PathBuf> = twin
        .files()
        .iter()
        .filter(|entry| is_modelica_file(&entry.relative_path))
        .filter_map(|entry| {
            let parent = entry.relative_path.parent()?.to_path_buf();
            Some(parent)
        })
        .collect();

    candidates.sort_by_key(|path| {
        (
            path.components().count(),
            lunco_assets::asset_path::slashed(path),
        )
    });
    candidates.dedup();

    let mut roots = Vec::new();
    for candidate in candidates {
        if roots
            .iter()
            .any(|root: &PathBuf| is_under_or_equal(&candidate, root))
        {
            continue;
        }
        roots.push(candidate);
    }
    roots
}

fn twin_for_root<'a>(
    workspace: Option<&'a lunco_workspace::WorkspaceResource>,
    root: &Path,
) -> Option<&'a lunco_twin::Twin> {
    workspace.and_then(|workspace| {
        workspace.twins().find_map(|(_, twin)| {
            (twin.root == root
                || twin
                    .root
                    .canonicalize()
                    .is_ok_and(|candidate| candidate == root))
            .then_some(twin)
        })
    })
}

/// Resolve one mounted Twin's Modelica source roots from the manifest and the
/// already-indexed files. The result contains only directories that exist in
/// the shared Twin asset authority; missing declarations are logged by the
/// caller rather than replaced with an unrelated fallback path.
fn twin_source_root_specs(
    twin_roots: &lunco_assets::twin_source::TwinRoots,
    name: &str,
    root: &Path,
    twin: Option<&lunco_twin::Twin>,
) -> Result<Vec<TwinSourceRootSpec>, String> {
    let manifest = if let Some(twin) = twin {
        twin.manifest.clone()
    } else {
        let path = root.join(lunco_twin::MANIFEST_FILENAME);
        if path.is_file() {
            Some(
                lunco_twin::TwinManifest::read(&path)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
            )
        } else {
            None
        }
    };
    let modelica = manifest
        .as_ref()
        .and_then(|manifest| manifest.modelica.as_ref());

    let local_paths = if let Some(modelica) = modelica {
        modelica.paths.clone()
    } else if let Some(twin) = twin {
        discover_twin_modelica_paths(twin)
    } else {
        Vec::new()
    };

    let mut specs = Vec::new();
    for path in local_paths {
        let relative = normalize_twin_source_path(&path)?;
        let Some(root_dir) = twin_roots
            .resolve_directory(name, &relative)
            .map_err(|error| format!("Twin `{name}` Modelica path lookup failed: {error}"))?
        else {
            log::error!(
                "[source-roots] Twin `{name}` Modelica path `{}` is not a directory",
                path.display()
            );
            continue;
        };
        if specs
            .iter()
            .all(|spec: &TwinSourceRootSpec| spec.root_dir != root_dir)
        {
            let index = specs.len();
            specs.push(TwinSourceRootSpec {
                id: format!("twin:{name}:local:{index}"),
                root_dir,
            });
        }
    }

    if let Some(modelica) = modelica {
        for external in &modelica.externals {
            let path = external.path.to_string_lossy();
            if path == "@bundled:msl" {
                continue;
            }
            if path.starts_with("@bundled:") {
                log::error!(
                    "[source-roots] Twin `{name}` external Modelica library `{}` is unsupported",
                    external.path.display()
                );
                continue;
            }
            let root_dir = if external.path.is_absolute() {
                external.path.clone()
            } else {
                root.join(&external.path)
            };
            if !root_dir.is_dir() {
                log::error!(
                    "[source-roots] Twin `{name}` external Modelica library `{}` is not a directory",
                    external.path.display()
                );
                continue;
            }
            if specs
                .iter()
                .all(|spec: &TwinSourceRootSpec| spec.root_dir != root_dir)
            {
                let index = specs.len();
                specs.push(TwinSourceRootSpec {
                    id: format!("twin:{name}:external:{index}"),
                    root_dir,
                });
            }
        }
    }

    Ok(specs)
}

/// Load a mounted Twin's Modelica packages into the compile session.
///
/// The Twin manifest owns explicit `[modelica].paths` and `externals`. When
/// that section is absent, this uses the already-indexed `.mo` files to find
/// package roots and flat source directories. Every result is dispatched to
/// the existing `LoadSourceRoot` worker command, so editor and runtime keep
/// one source-root admission path and one dependency/session view.
pub fn load_twin_source_roots(
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    channels: Option<Res<crate::ModelicaChannels>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut seen: Local<HashSet<String>>,
) {
    let (Some(twin_roots), Some(channels)) = (twin_roots, channels) else {
        return;
    };
    let names = match twin_roots.names() {
        Ok(names) => names,
        Err(error) => {
            log::error!("[source-roots] Twin registry unavailable: {error}");
            return;
        }
    };
    for name in names {
        if seen.contains(&name) {
            continue;
        }
        let root = match twin_roots.root_of(&name) {
            Ok(Some(root)) => root,
            Ok(None) => continue,
            Err(error) => {
                log::error!("[source-roots] Twin `{name}` lookup failed: {error}");
                return;
            }
        };
        let twin = twin_for_root(workspace.as_deref(), &root);
        let specs = match twin_source_root_specs(&twin_roots, &name, &root, twin) {
            Ok(specs) => specs,
            Err(error) => {
                log::error!("[source-roots] Twin `{name}` Modelica configuration failed: {error}");
                seen.insert(name);
                continue;
            }
        };
        // Record the twin even when it has no Modelica files. TwinRoots mutates
        // through interior handles and never triggers `Changed`; one terminal
        // probe is enough for this mounted root.
        seen.insert(name.clone());
        for spec in specs {
            if seen.contains(&spec.id) {
                continue;
            }
            let cmd = crate::worker::ModelicaCommand::LoadSourceRoot {
                id: spec.id.clone(),
                payload: crate::worker::LoadSourceRootPayload::Disk {
                    root_dir: spec.root_dir.clone(),
                },
            };
            if channels.tx.send(cmd).is_ok() {
                seen.insert(spec.id.clone());
                log::info!(
                    "[source-roots] loading Twin `{name}` Modelica source root `{}` from {}",
                    spec.id,
                    spec.root_dir.display()
                );
            }
        }
    }
}

pub fn ensure_loaded(
    registry: &mut SourceRootRegistry,
    id: &str,
    channels: &crate::ModelicaChannels,
) -> bool {
    let Some(entry) = registry.roots.get_mut(id) else {
        return false;
    };
    match &entry.state {
        LoadState::Ready => return true,
        LoadState::Loading { .. } => return false,
        LoadState::Failed(_) => return false,
        LoadState::NotLoaded => {}
    }
    // Build the payload + the human-readable summary for logging.
    // Each branch can fail early (e.g. missing bundled blob, unreadable
    // workspace file); on failure mark `Failed` and bail.
    let (payload, summary) = match &entry.kind {
        SourceRootKind::SystemLibrary {
            cache_subdir: _,
            root_dir,
        } => {
            let summary = format!("disk {}", root_dir.display());
            (
                crate::worker::LoadSourceRootPayload::Disk {
                    root_dir: root_dir.clone(),
                },
                summary,
            )
        }
        SourceRootKind::Bundled { filename } => {
            let Some(source) = crate::models::get_model(filename) else {
                bevy::log::warn!(
                    "[source-roots] bundled dep `{}` (file {}): not found \
                     in embedded models — leaving Failed",
                    id,
                    filename,
                );
                entry.state = LoadState::Failed(format!(
                    "bundled file `{}` missing from embedded models",
                    filename
                ));
                return false;
            };
            let summary = format!("bundled {}, {}B", filename, source.len());
            (
                crate::worker::LoadSourceRootPayload::InMemory {
                    label: format!("bundled:{filename}"),
                    files: vec![(filename.clone(), source.to_string())],
                },
                summary,
            )
        }
        SourceRootKind::BundledPackage { root } => {
            let files = lunco_assets::models::package_files_live(root);
            if files.is_empty() {
                bevy::log::warn!(
                    "[source-roots] bundled package `{}`: no Modelica files found",
                    root
                );
                entry.state = LoadState::Failed(format!(
                    "bundled Modelica package `{root}` has no source files"
                ));
                return false;
            }
            let summary = format!("bundled package {root}, {} files", files.len());
            (
                crate::worker::LoadSourceRootPayload::InMemory {
                    label: format!("bundled:{root}"),
                    files,
                },
                summary,
            )
        }
        SourceRootKind::WorkspaceFile { path } => {
            // Through `lunco-storage` (FileStorage native / WebStorage on wasm),
            // not `std::fs`: a workspace dependency can be opened in the web
            // build too, where the picked file's text lives in browser storage.
            let source = match crate::source_asset::read_text_sync(path) {
                Ok(s) => s,
                Err(e) => {
                    bevy::log::warn!(
                        "[source-roots] workspace file dep `{}` (path {}): \
                         read failed: {e} — leaving Failed",
                        id,
                        path.display(),
                    );
                    entry.state = LoadState::Failed(format!("workspace file read failed: {e}"));
                    return false;
                }
            };
            let uri = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("workspace.mo")
                .to_string();
            let summary = format!("workspace {}, {}B", path.display(), source.len());
            (
                crate::worker::LoadSourceRootPayload::InMemory {
                    label: format!("workspace:{}", path.display()),
                    files: vec![(uri, source)],
                },
                summary,
            )
        }
        SourceRootKind::SessionDocument { id: document_id } => {
            let message =
                format!("session document source root `{document_id}` has no standalone loader");
            bevy::log::error!("[source-roots] {message}");
            entry.state = LoadState::Failed(message);
            return false;
        }
    };

    // Dispatch + mark Loading. The worker's COMPILE lane is FIFO
    // (Steps of other live entities may jump ahead, but LoadSourceRoot /
    // Compile / Reset / UpdateParameters never reorder among themselves —
    // see `worker::enqueue_command`), so a Compile sent immediately after
    // this is guaranteed to see the loaded session. Worker results transition
    // Loading → Ready or Failed based on the actual load outcome.
    let cmd = crate::worker::ModelicaCommand::LoadSourceRoot {
        id: id.to_string(),
        payload,
    };
    if channels.tx.send(cmd).is_err() {
        bevy::log::warn!(
            "[source-roots] failed to dispatch LoadSourceRoot for `{}`: \
             worker channel closed",
            id,
        );
        entry.state = LoadState::Failed("worker channel closed".into());
        return false;
    }
    bevy::log::info!(
        "[source-roots] dispatched LoadSourceRoot `{}` ({}) to worker",
        id,
        summary,
    );
    entry.state = LoadState::Loading {
        progress: 0.0,
        started: Instant::now(),
    };
    // Status-bar feedback is projected from this `Loading` state by the reactive
    // UI observer `ui::core_observers::mirror_source_roots_to_status_bus`. Core
    // sets the state; it no longer touches the status bus.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn open_twin(root: &Path, manifest: &str) -> lunco_twin::Twin {
        fs::write(root.join(lunco_twin::MANIFEST_FILENAME), manifest).unwrap();
        match lunco_twin::TwinMode::open(root).unwrap() {
            lunco_twin::TwinMode::Twin(twin) => twin,
            other => panic!("expected a manifest-backed Twin, got {other:?}"),
        }
    }

    #[test]
    fn manifest_roots_use_twin_resolver_and_external_directories() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("models")).unwrap();
        fs::create_dir(temp.path().join("shared")).unwrap();
        let twin = open_twin(
            temp.path(),
            r#"
name = "demo"
version = "0.1.0"

[modelica]
paths = [".", "models"]
externals = [{ name = "Shared", path = "shared" }]
"#,
        );
        let roots = lunco_assets::twin_source::TwinRoots::default();
        let assigned = roots.register("demo", twin.root.clone()).unwrap();
        let specs = twin_source_root_specs(&roots, &assigned, &twin.root, Some(&twin)).unwrap();

        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].id, "twin:demo:local:0");
        assert_eq!(specs[0].root_dir, twin.root);
        assert_eq!(specs[1].id, "twin:demo:local:1");
        assert_eq!(specs[1].root_dir, twin.root.join("models"));
        assert_eq!(specs[2].id, "twin:demo:external:2");
        assert_eq!(specs[2].root_dir, twin.root.join("shared"));
    }

    #[test]
    fn absent_manifest_section_discovers_indexed_package_roots() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("models/Vehicle/Sub")).unwrap();
        fs::create_dir_all(temp.path().join("examples")).unwrap();
        fs::write(
            temp.path().join("models/Vehicle/package.mo"),
            "within ; package Vehicle end Vehicle;",
        )
        .unwrap();
        fs::write(
            temp.path().join("models/Vehicle/Sub/Part.mo"),
            "within Vehicle.Sub; model Part end Part;",
        )
        .unwrap();
        fs::write(
            temp.path().join("examples/Example.mo"),
            "model Example end Example;",
        )
        .unwrap();
        let twin = open_twin(temp.path(), "name = \"demo\"\nversion = \"0.1.0\"\n");
        let roots = lunco_assets::twin_source::TwinRoots::default();
        let assigned = roots.register("demo", twin.root.clone()).unwrap();
        let specs = twin_source_root_specs(&roots, &assigned, &twin.root, Some(&twin)).unwrap();

        assert_eq!(
            specs
                .iter()
                .map(|spec| spec.root_dir.clone())
                .collect::<Vec<_>>(),
            vec![twin.root.join("examples"), twin.root.join("models/Vehicle")]
        );
    }
}

/// Diagnostic log: walk the given AST, find every source-root
/// dependency, classify each against the registry, and emit a
/// one-line summary.
pub fn log_compile_deps(registry: &SourceRootRegistry, model_name: &str, ast: &StoredDefinition) {
    let deps = scan_source_root_deps(ast);
    if deps.is_empty() {
        bevy::log::info!(
            "[source-roots] compile `{}`: no external library deps",
            model_name,
        );
        return;
    }
    let mut ready = Vec::new();
    let mut not_loaded = Vec::new();
    let mut loading = Vec::new();
    let mut failed = Vec::new();
    let mut unknown = Vec::new();
    for root in &deps {
        match registry.state(root) {
            Some(LoadState::Ready) => ready.push(root.clone()),
            Some(LoadState::NotLoaded) => not_loaded.push(root.clone()),
            Some(LoadState::Loading { .. }) => loading.push(root.clone()),
            Some(LoadState::Failed(_)) => failed.push(root.clone()),
            None => unknown.push(root.clone()),
        }
    }
    bevy::log::info!(
        "[source-roots] compile `{}` deps: ready={:?} not_loaded={:?} \
         loading={:?} failed={:?} unknown={:?}",
        model_name,
        ready,
        not_loaded,
        loading,
        failed,
        unknown,
    );
}
