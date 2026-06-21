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
//!   workspace tree. Populated by a workspace scanner (PR-C).
//!
//! ## What this PR (PR-A) does
//!
//! Builds the inventory. No loads, no dep scanning, no gate. Every
//! entry starts in [`LoadState::NotLoaded`]. Subsequent PRs wire in:
//!  - PR-B: AST dep scanner + pre-compile gate + per-kind loaders.
//!  - PR-C: workspace file enumeration.
//!  - PR-D: status-bus mirror, retire `MslRemotePlugin`.
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
use std::path::PathBuf;
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
/// The dispatch in [`crate::source_roots`] PR-B's loader will match
/// on this enum to pick the right strategy. Kept inert here; only
/// metadata.
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
    /// User `.mo` file in the active workspace. Loaded by reading
    /// the file from disk and installing the resulting document.
    /// Populated by the PR-C workspace scanner.
    WorkspaceFile {
        /// Absolute path on disk.
        path: PathBuf,
    },
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
///
/// **Inventory only at this stage** — no loads run until PR-B's
/// pre-compile gate fires.
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
        for (cache_subdir, root_name) in
            crate::package_tree::scanner::discover_third_party_libs()
        {
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

        let lib_count = roots
            .values()
            .filter(|r| matches!(r.kind, SourceRootKind::SystemLibrary { .. }))
            .count();
        let bundled_count = roots
            .values()
            .filter(|r| matches!(r.kind, SourceRootKind::Bundled { .. }))
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
            None => SourceRootKind::Bundled {
                // Untitled docs don't have a filename; rumoca only
                // sees them via engine_resource sync. The `Bundled`
                // variant is a stand-in marker: never actually
                // loaded by the gate (state is already Ready).
                filename: format!("untitled:{id}"),
            },
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
/// PR-C strategy: this function publishes the source root's
/// location to the process-wide handle that
/// [`ModelicaCompiler::new`] consults via `preload_from_global`.
/// The handle store is a cheap `OnceLock` write; the **actual
/// parse cost** is paid inside the worker thread on its first
/// `ModelicaCompiler::new()` call. From the main thread's
/// perspective this function is microseconds.
///
/// Per-kind dispatch:
/// - [`SourceRootKind::SystemLibrary`] with `cache_subdir == "msl"`
///   → installs via [`lunco_assets::msl::install_global_msl_sources`].
///   The existing `MslRemotePlugin` plumbing handles the rest.
/// - Other system libraries (third-party) and Bundled / WorkspaceFile
///   → not yet supported; logs a warning and marks `Failed`. Their
///   compile path will surface the missing-type error from rumoca
///   the same way it did before PR-C. Adding support for these is
///   the work of a follow-up PR.
/// Source tag used for [`lunco_workbench::status_bus::StatusBus`]
/// progress entries during source-root loads.
pub const STATUS_BUS_SOURCE: &str = "source-roots";

pub fn ensure_loaded(
    registry: &mut SourceRootRegistry,
    id: &str,
    channels: &crate::ModelicaChannels,
    status_bus: Option<&mut lunco_workbench::status_bus::StatusBus>,
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
        SourceRootKind::SystemLibrary { cache_subdir: _, root_dir } => {
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
                    id, filename,
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
        SourceRootKind::WorkspaceFile { path } => {
            let source = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    bevy::log::warn!(
                        "[source-roots] workspace file dep `{}` (path {}): \
                         read failed: {e} — leaving Failed",
                        id, path.display(),
                    );
                    entry.state = LoadState::Failed(format!(
                        "workspace file read failed: {e}"
                    ));
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
    };

    // Dispatch + mark Loading. Worker is FIFO so a Compile sent
    // immediately after this is guaranteed to see the loaded
    // session. PR-D will add a result message to transition
    // Loading → Ready based on actual worker progress.
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
        id, summary,
    );
    entry.state = LoadState::Loading {
        progress: 0.0,
        started: Instant::now(),
    };
    if let Some(bus) = status_bus {
        bus.push_progress(
            STATUS_BUS_SOURCE,
            format!("Loading library `{id}`…"),
            0,
            0,
        );
        bus.push(
            STATUS_BUS_SOURCE,
            lunco_workbench::status_bus::StatusLevel::Info,
            format!("Loading library `{id}` ({summary})"),
        );
    }
    true
}

/// Diagnostic log: walk the given AST, find every source-root
/// dependency, classify each against the registry, and emit a
/// one-line summary.
pub fn log_compile_deps(
    registry: &SourceRootRegistry,
    model_name: &str,
    ast: &StoredDefinition,
) {
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
