//! Per-library / per-package source root registry.
//!
//! Inventory of every named source root the workbench knows how to
//! load into the rumoca compile session. A "source root" is any
//! qualified-path-root segment that compiles can depend on:
//!
//! - **Disk source roots**: any library or package explicitly registered by
//!   the application or an active Twin. They are loaded from their declared
//!   path; no cache-wide discovery is performed.
//! - **Bundled examples**: top-level `.mo` files supplied by the asset
//!   library. Loaded via
//!   [`lunco_assets_runtime::models::model_source`].
//! - **Workspace files**: user-authored `.mo` files in the active
//!   workspace tree.
//!
//! ## Design intent
//!
//! Generalises the source-bundle load path so that every source the compiler
//! needs goes through one registry with one state machine.
//! Adding a fourth system library, a new bundled example, or a
//! workspace folder becomes a data change, not new plumbing.

use bevy::prelude::*;
#[cfg(target_arch = "wasm32")]
use lunco_modelica_runtime::source_asset::read_text_sync;
use lunco_modelica_runtime::{LoadSourceRootPayload, ModelicaChannels, ModelicaCommand};
use rumoca_compile::parsing::ast::StoredDefinition;
use std::collections::HashMap;
use std::path::PathBuf;
use web_time::Instant;

/// Per-source-root state. Mirrors the `LibraryLoadState` shape, but
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
    /// On-disk source root explicitly registered by the application or Twin.
    Disk {
        /// Absolute path to the package root directory containing
        /// `package.mo`.
        root_dir: PathBuf,
    },
    /// Bundled example shipped inside the binary. Source bytes come
    /// from [`lunco_assets_runtime::models::model_source`]; install path is the same
    /// document-registry pipeline used when the user opens a bundled
    /// model from the package browser, but driven by the compile
    /// gate instead of a UI gesture.
    Bundled {
        /// `.mo` filename inside the runtime `models/` asset directory
        /// (e.g. `"AnnotatedRocketStage.mo"`).
        filename: String,
    },
    /// Structured Modelica package below the runtime `assets/models/<root>`.
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
    /// untitled document has no runtime asset filename and must never be sent
    /// through the bundled-model loader.
    SessionDocument { id: String },
}

/// One source root the workbench knows about. Keyed by the
/// qualified-path root segment (the value the dependency scanner extracts
/// from an external qualified reference).
#[derive(Debug, Clone)]
pub struct SourceRoot {
    /// Root segment of qualified names that resolve into this source.
    /// A package rooted at `Modelica` or another authored package name uses
    /// that name here; bundled examples use their own authored root.
    pub id: String,
    /// How to actually load this root when the gate decides to.
    pub kind: SourceRootKind,
    /// Current load state. Transitions:
    /// `NotLoaded` → `Loading` (gate kicks off bg task)
    /// `Loading` → `Ready` / `Failed` (loader completes).
    pub state: LoadState,
}

/// Process-wide registry of every named source root. Owned by the
/// Modelica host/application; populated at host start by inventorying:
///  - Bundled examples via [`lunco_assets_runtime::models::model_filenames`].
///  - Structured packages via [`lunco_assets_runtime::models::package_roots_live`].
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

        // Bundled examples — keyed by filename stem (the convention
        // every bundled `.mo` follows: `<Root>.mo` contains `package <Root>`
        // or `model <Root>`). The dep-scanner extracts the root from a
        // `Foo.X` reference and looks it up here.
        let bundled_models = match lunco_assets_runtime::models::model_filenames() {
            Ok(filenames) => filenames,
            Err(error) => {
                bevy::log::error!("[source-roots] Modelica example inventory failed: {error}");
                Vec::new()
            }
        };
        for filename in bundled_models {
            let Some(id) = filename.strip_suffix(".mo") else {
                continue;
            };
            // Keep the first explicit registration authoritative.
            if roots.contains_key(id) {
                continue;
            }
            roots.insert(
                id.to_string(),
                SourceRoot {
                    id: id.to_string(),
                    kind: SourceRootKind::Bundled { filename },
                    state: LoadState::NotLoaded,
                },
            );
        }

        // Structured packages — keyed by their Modelica root segment. On
        // native, prefer the live package directory so editor changes are
        // visible without rebuilding; browser consumers use the Bevy asset path.
        // This is the standard root-segment search-path inventory, not a
        // library-specific registration.
        let package_roots = match lunco_assets_runtime::models::package_roots_live() {
            Ok(roots) => roots,
            Err(error) => {
                bevy::log::error!("[source-roots] Modelica package inventory failed: {error}");
                Vec::new()
            }
        };
        for root_name in package_roots {
            if roots.contains_key(&root_name) {
                continue;
            }
            let kind = lunco_assets_core::models_package_root_path(&root_name)
                .map(|root_dir| SourceRootKind::Disk { root_dir })
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
            .filter(|r| matches!(r.kind, SourceRootKind::Disk { .. }))
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
            "[source-roots] registry built: {} disk roots, {} bundled examples \
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

    /// Register one explicitly configured disk source root.
    pub fn register_disk_root(&mut self, id: impl Into<String>, root_dir: PathBuf) {
        let id = id.into();
        self.roots.insert(
            id.clone(),
            SourceRoot {
                id,
                kind: SourceRootKind::Disk { root_dir },
                state: LoadState::NotLoaded,
            },
        );
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
    /// existing `kind` if it's an explicit disk root (a workspace doc
    /// must not shadow it), otherwise overwrites with the new
    /// metadata. Always flips state to `Ready`.
    pub fn register_open_doc_root(&mut self, id: String, path: Option<PathBuf>) {
        // Don't let an opened document shadow an explicit disk root.
        if let Some(existing) = self.roots.get(&id) {
            if matches!(existing.kind, SourceRootKind::Disk { .. }) {
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

/// Installs source-root inventory and document discovery for every Modelica
/// host, including headless execution. The inventory is application state;
/// editor panels only present its load lifecycle.
pub struct ModelicaSourceRootsPlugin;

impl Plugin for ModelicaSourceRootsPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<SourceRootRegistry>() {
            app.insert_resource(SourceRootRegistry::build());
        }
        app.add_observer(register_open_document_source_root);
        register_twin_modelica_commands(app);
    }
}

/// Whether a document is already represented in a loaded source library and
/// must therefore be compiled by its canonical class name without overlaying
/// its source a second time.
pub fn is_library_document(document: &lunco_modelica_document::ModelicaDocument) -> bool {
    match document.origin() {
        lunco_doc::DocumentOrigin::File { path, writable } => {
            !writable || lunco_assets_runtime::library::owns_filesystem_path(path)
        }
        _ => false,
    }
}

/// Source text to overlay for a document compile. Library documents are
/// already installed in the compiler session; ordinary documents need their
/// exact source snapshot overlaid.
pub fn compile_overlay_source(document: &lunco_modelica_document::ModelicaDocument) -> String {
    if is_library_document(document) {
        String::new()
    } else {
        document.source().to_string()
    }
}

/// Register top-level source roots contributed by an opened Modelica document.
///
/// This observer belongs to the source-root host rather than the document core:
/// opening a document is a generic lifecycle event, while deciding that its
/// top-level classes are compiler source roots is a Modelica admission policy.
pub fn register_open_document_source_root(
    trigger: On<lunco_doc_bevy::DocumentOpened>,
    registry: Res<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>,
    source_roots: Option<ResMut<SourceRootRegistry>>,
) {
    let Some(mut source_roots) = source_roots else {
        return;
    };
    let id = trigger.event().doc;
    let Some(host) = registry.host(id) else {
        return;
    };
    let document = host.document();
    let path = match document.origin() {
        lunco_doc::DocumentOrigin::File { path, .. } => Some(path.clone()),
        _ => None,
    };
    for class in document.index().classes.values() {
        if !class.name.contains('.') {
            source_roots.register_open_doc_root(class.name.clone(), path.clone());
        }
    }
}

/// Walk an AST and extract the set of qualified-path root segments
/// that the AST references. The result is the input to the load
/// gate: each segment is looked up in [`SourceRootRegistry`] to
/// decide whether the corresponding source root needs to be loaded
/// before compile.
///
/// For example, an AST that contains qualified references rooted at
/// `Control.Blocks.Interfaces.RealOutput` and
/// `extends Thermal.Boundaries.Base` yields `{"Control", "Thermal"}`.
///
/// Filters out:
/// - Built-in scalar types (`Real`, `Integer`, etc.) — handled by
///   rumoca natively.
/// - Bare (non-qualified) names — those resolve locally via the
///   doc's own classes, no external load needed.
/// - The empty string (defensive).
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
/// Source tag used for [`lunco_status_core::status_bus::StatusBus`]
/// progress entries during source-root loads.
pub const STATUS_BUS_SOURCE: &str = "source-roots";

/// Load one Twin-selected Modelica directory through the shared worker root
/// pipeline. Rhai owns which manifest/index paths to request; this command owns
/// only path admission and registration of the generic disk source.
#[lunco_core::Command(default)]
pub struct LoadTwinModelicaSourceRoot {
    /// Workspace identity of the Twin that declared the source root.
    pub twin_id: u64,
    /// Exact `twin://` authority returned by the asset owner.
    pub name: String,
    /// Twin-scoped source-root key, beginning with `twin:<id>:`.
    pub id: String,
    /// Twin-relative directory or an explicitly declared absolute external path.
    pub path: String,
}

#[lunco_core::on_command(LoadTwinModelicaSourceRoot)]
fn on_load_twin_modelica_source_root(
    trigger: bevy::ecs::observer::On<LoadTwinModelicaSourceRoot>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    channels: Option<Res<ModelicaChannels>>,
    registry: Option<ResMut<SourceRootRegistry>>,
) -> Result<lunco_command_contracts::Ack, String> {
    let request = trigger.event();
    let twin_id = lunco_workspace::TwinId::new(request.twin_id);
    let twin = workspace
        .as_deref()
        .and_then(|workspace| workspace.twin(twin_id))
        .ok_or_else(|| format!("workspace Twin {} is unavailable", request.twin_id))?;
    if workspace
        .as_deref()
        .is_none_or(|workspace| workspace.active_twin != Some(twin_id))
    {
        return Err(format!("Twin {} is not active", request.twin_id));
    }
    let id_prefix = format!("twin:{}:", request.name);
    if !request.id.starts_with(&id_prefix) || request.id.trim() == id_prefix {
        return Err(format!(
            "Modelica source-root id `{}` must be scoped to Twin authority `{}`",
            request.id, request.name
        ));
    }
    let authority_root = roots
        .as_deref()
        .and_then(|roots| roots.name_for_root(&twin.root).ok().flatten())
        .ok_or_else(|| format!("Twin asset authority `{}` is unavailable", request.name))?;
    if authority_root != request.name {
        return Err(format!(
            "Twin asset authority `{}` does not belong to Twin {}",
            request.name, request.twin_id
        ));
    }

    let authored_path = PathBuf::from(&request.path);
    let root_dir = if authored_path.is_absolute() {
        let declared = twin
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.modelica.as_ref())
            .is_some_and(|modelica| {
                modelica
                    .externals
                    .iter()
                    .any(|external| external.path == authored_path)
            });
        if !declared {
            return Err(format!(
                "absolute Modelica source root `{}` is not declared in `[modelica].externals`",
                authored_path.display()
            ));
        }
        #[cfg(target_arch = "wasm32")]
        return Err("absolute Modelica source roots are unavailable in the browser".into());
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !authored_path.is_dir() {
                return Err(format!(
                    "absolute Modelica source root `{}` is not a directory",
                    authored_path.display()
                ));
            }
            authored_path
        }
    } else {
        if request.path.contains('\\') || !lunco_twin::is_safe_relative_path(&authored_path) {
            return Err(format!(
                "Modelica source path `{}` must stay within the Twin root",
                request.path
            ));
        }
        roots
            .as_deref()
            .ok_or_else(|| "TwinRoots is not installed".to_owned())?
            .resolve_directory(&request.name, &authored_path)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Twin Modelica directory `{}` is unavailable", request.path))?
    };

    let channels = channels.ok_or_else(|| "Modelica worker is not installed".to_owned())?;
    let mut registry =
        registry.ok_or_else(|| "Modelica source-root registry is not installed".to_owned())?;
    if let Some(existing) = registry.roots.get(&request.id) {
        match &existing.kind {
            SourceRootKind::Disk {
                root_dir: existing_path,
            } if existing_path == &root_dir => match &existing.state {
                LoadState::Ready | LoadState::Loading { .. } => {
                    return Ok(lunco_command_contracts::Ack::new(
                        lunco_command_contracts::OpId::new(),
                    ));
                }
                LoadState::Failed(error) => {
                    return Err(format!(
                        "Modelica source root `{}` failed: {error}",
                        request.id
                    ));
                }
                LoadState::NotLoaded => {}
            },
            _ => {
                return Err(format!(
                    "Modelica source-root id `{}` is already assigned to another source",
                    request.id
                ));
            }
        }
    } else {
        registry.register_disk_root(request.id.clone(), root_dir);
    }

    if !ensure_loaded(&mut registry, &request.id, &channels) {
        let detail = match registry.state(&request.id) {
            Some(LoadState::Loading { .. }) | Some(LoadState::Ready) => None,
            Some(LoadState::Failed(error)) => Some(error.clone()),
            _ => Some("source-root load could not be queued".to_owned()),
        };
        if let Some(detail) = detail {
            return Err(format!("Modelica source root `{}`: {detail}", request.id));
        }
    }
    Ok(lunco_command_contracts::Ack::new(
        lunco_command_contracts::OpId::new(),
    ))
}

lunco_core::register_commands!(on_load_twin_modelica_source_root);

/// Register the typed command used by authored Twin loading policies.
pub fn register_twin_modelica_commands(app: &mut App) {
    register_all_commands(app);
}

pub fn ensure_loaded(
    registry: &mut SourceRootRegistry,
    id: &str,
    channels: &ModelicaChannels,
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
    // Native workers resolve/read source files during immutable preparation.
    // The browser keeps its storage reads on the host that owns WebStorage and
    // sends the resulting text to the Modelica Web Worker.
    let (payload, summary) = match &entry.kind {
        SourceRootKind::Disk { root_dir } => {
            let summary = format!("disk {}", root_dir.display());
            (
                LoadSourceRootPayload::Disk {
                    root_dir: root_dir.clone(),
                },
                summary,
            )
        }
        SourceRootKind::Bundled { filename } => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                (
                    LoadSourceRootPayload::BundledModel {
                        filename: filename.clone(),
                    },
                    format!("bundled {filename}"),
                )
            }
            #[cfg(target_arch = "wasm32")]
            {
                let source = match lunco_assets_runtime::models::model_source(filename) {
                    Ok(Some(source)) => source,
                    Ok(None) => {
                        let error = format!("Modelica asset `{filename}` was not found");
                        bevy::log::warn!("[source-roots] {error}");
                        entry.state = LoadState::Failed(error);
                        return false;
                    }
                    Err(error) => {
                        bevy::log::warn!("[source-roots] cannot load `{filename}`: {error}");
                        entry.state = LoadState::Failed(error);
                        return false;
                    }
                };
                (
                    LoadSourceRootPayload::InMemory {
                        label: format!("bundled:{filename}"),
                        files: vec![(filename.clone(), source.to_string())],
                    },
                    format!("bundled {filename}, {}B", source.len()),
                )
            }
        }
        SourceRootKind::BundledPackage { root } => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                (
                    LoadSourceRootPayload::BundledPackage { root: root.clone() },
                    format!("bundled package {root}"),
                )
            }
            #[cfg(target_arch = "wasm32")]
            {
                let files = match lunco_assets_runtime::models::package_files_live(root) {
                    Ok(files) if !files.is_empty() => files,
                    Ok(_) => {
                        let error = format!("Modelica package `{root}` has no source files");
                        bevy::log::warn!("[source-roots] {error}");
                        entry.state = LoadState::Failed(error);
                        return false;
                    }
                    Err(error) => {
                        bevy::log::warn!("[source-roots] cannot load package `{root}`: {error}");
                        entry.state = LoadState::Failed(error);
                        return false;
                    }
                };
                let file_count = files.len();
                (
                    LoadSourceRootPayload::InMemory {
                        label: format!("bundled:{root}"),
                        files,
                    },
                    format!("bundled package {root}, {file_count} files"),
                )
            }
        }
        SourceRootKind::WorkspaceFile { path } => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                (
                    LoadSourceRootPayload::WorkspaceFile { path: path.clone() },
                    format!("workspace {}", path.display()),
                )
            }
            #[cfg(target_arch = "wasm32")]
            {
                let source = match read_text_sync(path) {
                    Ok(source) => source,
                    Err(error) => {
                        let detail = format!(
                            "workspace file dep `{}` (path {}) read failed: {error}",
                            id,
                            path.display(),
                        );
                        bevy::log::warn!("[source-roots] {detail}");
                        entry.state = LoadState::Failed(detail);
                        return false;
                    }
                };
                let uri = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("workspace.mo")
                    .to_string();
                (
                    LoadSourceRootPayload::InMemory {
                        label: format!("workspace:{}", path.display()),
                        files: vec![(uri, source.clone())],
                    },
                    format!("workspace {}, {}B", path.display(), source.len()),
                )
            }
        }
        SourceRootKind::SessionDocument { id: document_id } => {
            let message =
                format!("session document source root `{document_id}` has no standalone loader");
            bevy::log::error!("[source-roots] {message}");
            entry.state = LoadState::Failed(message);
            return false;
        }
    };

    // The native worker prepares file-backed roots on its bounded pool, then
    // commits them to the session in command order. Dependent compile commands
    // remain queued until those commits return. The Web Worker owns its parse
    // and session-install path after receiving the in-memory payload.
    let cmd = ModelicaCommand::LoadSourceRoot {
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

/// Admit the known source roots required by a compile before that compile is
/// sent to the worker. Root requests and compile requests share one ordered
/// channel; the worker holds compilation until all admitted root preparations
/// have committed. Unknown or failed roots are terminal here instead of
/// falling through to synchronous compiler-side discovery.
pub fn admit_compile_roots(
    registry: &mut SourceRootRegistry,
    roots: impl IntoIterator<Item = String>,
    channels: &ModelicaChannels,
) -> Result<(), String> {
    let roots = roots.into_iter().collect::<std::collections::BTreeSet<_>>();
    for id in &roots {
        let Some(state) = registry.state(id) else {
            return Err(format!("Modelica source root `{id}` is not registered"));
        };
        if let LoadState::Failed(error) = state {
            return Err(format!("Modelica source root `{id}` failed: {error}"));
        }
    }
    for id in roots {
        ensure_loaded(registry, &id, channels);
        match registry.state(&id) {
            Some(LoadState::Ready | LoadState::Loading { .. }) => {}
            Some(LoadState::Failed(error)) => {
                return Err(format!("Modelica source root `{id}` failed: {error}"));
            }
            Some(LoadState::NotLoaded) => {
                return Err(format!(
                    "Modelica source root `{id}` could not be queued for loading"
                ));
            }
            None => {
                return Err(format!(
                    "Modelica source root `{id}` was removed during admission"
                ));
            }
        }
    }
    Ok(())
}

/// Diagnostic log: walk the given AST, find every source-root
/// dependency, classify each against the registry, and emit a
/// one-line summary.
pub fn log_compile_deps(registry: &SourceRootRegistry, model_name: &str, ast: &StoredDefinition) {
    let deps = lunco_modelica_index::source_deps::scan_source_root_deps(ast);
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
