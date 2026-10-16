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
#[cfg(not(target_arch = "wasm32"))]
use lunco_assets_runtime::library::LibraryLoadState;
#[cfg(target_arch = "wasm32")]
use lunco_modelica_runtime::source_asset::read_text_sync;
use lunco_modelica_runtime::{LoadSourceRootPayload, ModelicaChannels, ModelicaCommand};
use rumoca_compile::parsing::ast::StoredDefinition;
use std::collections::{HashMap, VecDeque};
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
    /// The installed library artifact, with namespaces from its declaration index.
    SourceLibrary,
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

/// One admitted source set. Its identity is distinct from the namespaces
/// declared by its parsed sources.
#[derive(Debug, Clone)]
pub struct SourceRoot {
    /// Stable registration identity; document and Twin sets include their owner.
    pub id: String,
    /// Parsed authored namespaces installed by the current load operation.
    /// Scoped source-set IDs remain the authoritative lifetime owner.
    pub namespaces: Vec<String>,
    /// Per-file facts from admission and saved editor generations.
    pub source_files: Vec<lunco_modelica_ast::resource_uri::SourceFileFacts>,
    /// Current document generations overlaid at this same source-set owner.
    pub document_sources:
        HashMap<lunco_doc::DocumentId, lunco_modelica_ast::resource_uri::SourceFileFacts>,
    /// Saves which must survive an older in-flight source-set admission.
    pending_saved_sources: HashMap<String, lunco_modelica_ast::resource_uri::SourceFileFacts>,
    pub origin_document: Option<lunco_doc::DocumentId>,
    /// How to actually load this root when the gate decides to.
    pub kind: SourceRootKind,
    /// Scope that owns this registration. Shared application roots survive
    /// Twin close; Twin roots are removed at that ownership boundary.
    pub owner: SourceRootOwner,
    /// Latest operation issued for this root id.
    pub operation_id: u64,
    /// Current load state. Transitions:
    /// `NotLoaded` → `Loading` (gate kicks off bg task)
    /// `Loading` → `Ready` / `Failed` (loader completes).
    pub state: LoadState,
}

impl SourceRoot {
    pub fn effective_sources(&self) -> Vec<&lunco_modelica_ast::resource_uri::SourceFileFacts> {
        let overridden = self
            .document_sources
            .values()
            .map(|file| file.source_uri.as_str())
            .collect::<std::collections::HashSet<_>>();
        let mut files = self
            .source_files
            .iter()
            .filter(|file| !overridden.contains(file.source_uri.as_str()))
            .chain(self.document_sources.values())
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.source_uri.cmp(&right.source_uri));
        files
    }

    fn refresh_document_source(
        &mut self,
        id: lunco_doc::DocumentId,
        facts: lunco_modelica_ast::resource_uri::SourceFileFacts,
        saved: bool,
    ) {
        if saved {
            if matches!(self.state, LoadState::NotLoaded | LoadState::Loading { .. }) {
                self.pending_saved_sources
                    .insert(facts.source_uri.clone(), facts.clone());
            }
            self.source_files
                .retain(|file| file.source_uri != facts.source_uri);
            self.source_files.push(facts.clone());
        }
        self.document_sources.insert(id, facts);
    }

    fn effective_namespaces(&self) -> Vec<String> {
        // A document registers one entry per top-level namespace. Its file
        // facts may contain several declarations; sibling entries must not
        // claim each other's namespaces. Directory sets admit all their files.
        if self.origin_document.is_some() {
            return self.namespaces.clone();
        }
        let files = self.effective_sources();
        if files.is_empty() {
            return self.namespaces.clone();
        }
        let mut names = files
            .iter()
            .flat_map(|file| &file.classes)
            .filter_map(|name| {
                rumoca_core::qualified_name_segments(name)
                    .next()
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names
    }
}

/// Lifetime owner for one compiler source set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRootOwner {
    Application,
    Twin(lunco_workspace::TwinId),
    Replicated(lunco_workspace::ReplicationOwner),
}

/// Process-wide registry of every named source root. Owned by the
/// Modelica host/application; populated at host start by inventorying:
///  - Bundled examples via [`lunco_assets_runtime::models::model_filenames`].
///  - Structured packages via [`lunco_assets_runtime::models::package_roots_live`].
///
/// Loading remains demand-driven: inventory is cheap, and a root is installed
/// only when a compile or class lookup actually references it.
#[derive(Resource, Debug, Clone)]
pub struct SourceRootRegistry {
    /// Map of root id → entry. The dep-scanner looks up qualified-
    /// path roots here; the gate transitions state on each entry.
    pub roots: HashMap<String, SourceRoot>,
    next_operation_id: u64,
}

impl Default for SourceRootRegistry {
    fn default() -> Self {
        Self {
            roots: HashMap::new(),
            next_operation_id: 1,
        }
    }
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
                    namespaces: Vec::new(),
                    source_files: Vec::new(),
                    document_sources: HashMap::new(),
                    pending_saved_sources: HashMap::new(),
                    origin_document: None,
                    kind: SourceRootKind::Bundled { filename },
                    owner: SourceRootOwner::Application,
                    operation_id: 0,
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
            let kind = match lunco_assets_core::models_package_root_path(&root_name) {
                Ok(Some(root_dir)) => SourceRootKind::Disk { root_dir },
                Ok(None) => SourceRootKind::BundledPackage {
                    root: root_name.clone(),
                },
                Err(error) => {
                    bevy::log::error!("[source-roots] model root `{root_name}` failed: {error}");
                    continue;
                }
            };
            roots.insert(
                root_name.clone(),
                SourceRoot {
                    id: root_name,
                    namespaces: Vec::new(),
                    source_files: Vec::new(),
                    document_sources: HashMap::new(),
                    pending_saved_sources: HashMap::new(),
                    origin_document: None,
                    kind,
                    owner: SourceRootOwner::Application,
                    operation_id: 0,
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

        Self {
            roots,
            next_operation_id: 1,
        }
    }

    /// Register one explicitly configured disk source root.
    pub fn register_disk_root(&mut self, id: impl Into<String>, root_dir: PathBuf) {
        let id = id.into();
        self.roots.insert(
            id.clone(),
            SourceRoot {
                id,
                namespaces: Vec::new(),
                source_files: Vec::new(),
                document_sources: HashMap::new(),
                pending_saved_sources: HashMap::new(),
                origin_document: None,
                kind: SourceRootKind::Disk { root_dir },
                owner: SourceRootOwner::Application,
                operation_id: 0,
                state: LoadState::NotLoaded,
            },
        );
    }

    /// Register a source root selected by a Twin's authored lifecycle policy.
    pub fn register_twin_disk_root(
        &mut self,
        id: impl Into<String>,
        root_dir: PathBuf,
        twin: lunco_workspace::TwinId,
    ) {
        let id = id.into();
        self.roots.insert(
            id.clone(),
            SourceRoot {
                id,
                namespaces: Vec::new(),
                source_files: Vec::new(),
                document_sources: HashMap::new(),
                pending_saved_sources: HashMap::new(),
                origin_document: None,
                kind: SourceRootKind::Disk { root_dir },
                owner: SourceRootOwner::Twin(twin),
                operation_id: 0,
                state: LoadState::NotLoaded,
            },
        );
    }

    /// Register a document-backed namespace without replacing an admitted asset
    /// root. Document facts carry their exact owner and retire with that document.
    pub fn register_open_doc_root(
        &mut self,
        namespace: String,
        path: Option<PathBuf>,
        document: lunco_doc::DocumentId,
        owner: SourceRootOwner,
    ) -> Option<String> {
        // A document view does not replace an explicitly admitted asset root.
        if let Some(existing) = self.roots.get(&namespace) {
            if existing.origin_document.is_none() {
                return None;
            }
        }
        let id = format!("document:{document}:{namespace}");
        let kind = match path {
            Some(p) => SourceRootKind::WorkspaceFile { path: p },
            None => SourceRootKind::SessionDocument {
                id: namespace.clone(),
            },
        };
        self.roots.insert(
            id.clone(),
            SourceRoot {
                id: id.clone(),
                namespaces: vec![namespace],
                source_files: Vec::new(),
                document_sources: HashMap::new(),
                pending_saved_sources: HashMap::new(),
                origin_document: Some(document),
                kind,
                owner,
                operation_id: 0,
                state: LoadState::Ready,
            },
        );
        Some(id)
    }

    /// Borrow an entry's load state.
    pub fn state(&self, id: &str) -> Option<&LoadState> {
        self.roots.get(id).map(|r| &r.state)
    }

    /// Commit parsed namespace facts only for the currently admitted load.
    /// Stale worker results cannot revive a retired/replaced source set.
    pub fn complete_load(
        &mut self,
        id: &str,
        operation_id: u64,
        mut namespaces: Vec<String>,
        source_files: Vec<lunco_modelica_ast::resource_uri::SourceFileFacts>,
        error: Option<String>,
    ) -> bool {
        let Some(entry) = self
            .roots
            .get_mut(id)
            .filter(|entry| entry.operation_id == operation_id)
        else {
            return false;
        };
        namespaces.sort_unstable();
        namespaces.dedup();
        let error = error.or_else(|| {
            (namespaces.is_empty() || namespaces.iter().any(|name| name.trim().is_empty())).then(
                || {
                    "source-root installation returned no valid authored namespace identities"
                        .to_owned()
                },
            )
        });
        match error {
            Some(error) => {
                entry.namespaces.clear();
                entry.source_files.clear();
                entry.pending_saved_sources.clear();
                entry.state = LoadState::Failed(error);
            }
            None => {
                entry.namespaces = namespaces;
                entry.source_files = source_files;
                for (uri, facts) in entry.pending_saved_sources.drain() {
                    entry.source_files.retain(|file| file.source_uri != uri);
                    entry.source_files.push(facts);
                }
                entry
                    .source_files
                    .sort_by(|left, right| left.source_uri.cmp(&right.source_uri));
                entry.state = LoadState::Ready;
            }
        }
        true
    }

    /// Resolve a qualified Modelica root to its unique source-set owner.
    /// Registration IDs identify ordinary application roots before first load;
    /// completed parsed namespaces identify Twin-scoped source sets.
    pub fn resolve_source_root(&self, namespace: &str) -> Result<String, String> {
        let mut owners = self
            .roots
            .iter()
            .filter_map(|(id, entry)| {
                (id == namespace
                    || entry
                        .effective_namespaces()
                        .iter()
                        .any(|name| name == namespace))
                .then(|| id.clone())
            })
            .collect::<Vec<_>>();
        owners.sort_unstable();
        match owners.as_slice() {
            [id] => Ok(id.clone()),
            [] => Err(format!(
                "Modelica source root `{namespace}` is not registered"
            )),
            _ => Err(format!(
                "Modelica namespace `{namespace}` has multiple registered source-set owners: {}",
                owners
                    .iter()
                    .map(|id| match &self.roots[id].kind {
                        SourceRootKind::WorkspaceFile { path } => path.display().to_string(),
                        SourceRootKind::Disk { root_dir } => root_dir.display().to_string(),
                        SourceRootKind::SessionDocument { id } =>
                            format!("unsaved document `{id}`"),
                        SourceRootKind::Bundled { filename } =>
                            lunco_assets_core::engine_asset_uri(&format!("models/{filename}")),
                        SourceRootKind::BundledPackage { root } =>
                            lunco_assets_core::engine_asset_uri(&format!("models/{root}")),
                        SourceRootKind::SourceLibrary => "installed source library".to_owned(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    fn allocate_operation_id(&mut self) -> Result<u64, String> {
        let id = self.next_operation_id;
        self.next_operation_id = id
            .checked_add(1)
            .ok_or_else(|| "Modelica source-root operation identity exhausted".to_owned())?;
        Ok(id)
    }

    /// Remove the registry entries owned by `twin` and allocate their ordered
    /// unload operations. Application roots and other Twin entries are retained.
    pub fn take_twin_roots(
        &mut self,
        twin: lunco_workspace::TwinId,
    ) -> Result<Vec<(String, u64)>, String> {
        let mut ids = self
            .roots
            .iter()
            .filter(|(_, root)| root.owner == SourceRootOwner::Twin(twin))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        let operation_ids = (0..ids.len())
            .map(|_| self.allocate_operation_id())
            .collect::<Result<Vec<_>, _>>()?;
        for id in &ids {
            self.roots.remove(id);
        }
        Ok(ids.into_iter().zip(operation_ids).collect())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
pub struct NativeLibraryIndexLoad {
    task: Option<
        bevy::tasks::Task<Result<lunco_modelica_index::visual_diagram::LibraryIndex, String>>,
    >,
    failed: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeLibraryIndexLoad {
    pub fn new() -> Self {
        Self {
            task: None,
            failed: false,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn drive_native_library_index(
    index_load: Option<ResMut<NativeLibraryIndexLoad>>,
    state: Option<ResMut<LibraryLoadState>>,
    mut commands: Commands,
) {
    use bevy::tasks::futures_lite::future;

    let Some(mut index_load) = index_load else {
        return;
    };

    if index_load.failed || lunco_modelica_index::visual_diagram::library_index_available() {
        return;
    }

    if index_load.task.is_some() {
        let result = {
            let task = index_load
                .task
                .as_mut()
                .expect("source library index task disappeared while being polled");
            future::block_on(future::poll_once(task))
        };
        let Some(result) = result else {
            return;
        };
        index_load.task = None;
        match result {
            Ok(index) => {
                if lunco_modelica_index::visual_diagram::install_library_index(index) {
                    info!("[source library] editor index loaded off-thread");
                    commands.trigger(
                        lunco_modelica_index::visual_diagram::LibraryEditorIndexBecameReady,
                    );
                }
            }
            Err(error) => {
                index_load.failed = true;
                error!("[source library] editor index load failed: {error}");
                if let Some(mut state) = state {
                    *state = LibraryLoadState::Failed(error);
                }
            }
        }
        return;
    }

    if !state.is_some_and(|state| state.is_ready()) {
        return;
    }

    info!("[source library] loading editor index off-thread");
    index_load.task =
        Some(bevy::tasks::AsyncComputeTaskPool::get().spawn(async {
            lunco_modelica_index::visual_diagram::load_library_index_from_assets()
        }));
}

/// Publish installed library namespaces from its parsed declaration index.
fn register_indexed_library_source_root(
    _trigger: On<lunco_modelica_index::visual_diagram::LibraryEditorIndexBecameReady>,
    mut registry: ResMut<SourceRootRegistry>,
) {
    let namespaces = lunco_modelica_index::visual_diagram::library_class_library()
        .iter()
        .filter_map(|class| {
            rumoca_core::qualified_name_segments(&class.name)
                .next()
                .map(str::to_owned)
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if namespaces.is_empty() {
        bevy::log::error!("[source-roots] installed library index has no declared namespaces");
        return;
    }
    let id = "source-bundle";
    registry.roots.insert(
        id.to_owned(),
        SourceRoot {
            id: id.to_owned(),
            namespaces,
            source_files: Vec::new(),
            document_sources: HashMap::new(),
            pending_saved_sources: HashMap::new(),
            origin_document: None,
            kind: SourceRootKind::SourceLibrary,
            owner: SourceRootOwner::Application,
            operation_id: 0,
            state: LoadState::NotLoaded,
        },
    );
}

#[derive(Resource, Default)]
struct PendingSourceRootUnloads {
    queued: VecDeque<(String, u64)>,
    terminal_error: Option<String>,
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
        app.init_resource::<PendingSourceRootUnloads>();
        app.init_resource::<resources::ResourceContextHandle>();
        app.add_systems(
            Update,
            resources::publish_resource_context.run_if(resource_changed::<SourceRootRegistry>),
        );
        app.add_observer(register_open_document_source_root);
        app.add_observer(refresh_document_source_roots);
        app.add_observer(refresh_saved_document_source_roots);
        app.add_observer(retire_document_source_roots);
        app.add_observer(register_indexed_library_source_root);
        #[cfg(not(target_arch = "wasm32"))]
        {
            app.add_systems(Update, drive_native_library_index);
            if lunco_assets_runtime::library::has_library_source() {
                app.init_resource::<NativeLibraryIndexLoad>();
            }
        }
        app.add_observer(unload_twin_modelica_source_roots);
        app.add_systems(PreUpdate, flush_pending_source_root_unloads);
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
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
) {
    refresh_document_roots(
        trigger.event().doc,
        &registry,
        source_roots,
        workspace.as_deref(),
    );
}

fn refresh_document_source_roots(
    trigger: On<lunco_doc_bevy::DocumentChanged>,
    registry: Res<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>,
    source_roots: Option<ResMut<SourceRootRegistry>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
) {
    refresh_document_roots(
        trigger.event().doc,
        &registry,
        source_roots,
        workspace.as_deref(),
    );
}

fn refresh_saved_document_source_roots(
    trigger: On<lunco_doc_bevy::DocumentSaved>,
    registry: Res<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>,
    source_roots: Option<ResMut<SourceRootRegistry>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
) {
    refresh_document_roots(
        trigger.event().doc,
        &registry,
        source_roots,
        workspace.as_deref(),
    );
}

fn retire_document_source_roots(
    trigger: On<lunco_doc_bevy::DocumentClosed>,
    mut roots: ResMut<SourceRootRegistry>,
) {
    roots
        .roots
        .retain(|_, root| root.origin_document != Some(trigger.event().doc));
    for root in roots.roots.values_mut() {
        root.document_sources.remove(&trigger.event().doc);
    }
}

fn refresh_document_roots(
    id: lunco_doc::DocumentId,
    registry: &lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>,
    source_roots: Option<ResMut<SourceRootRegistry>>,
    workspace: Option<&lunco_workspace::WorkspaceResource>,
) {
    let Some(mut source_roots) = source_roots else {
        return;
    };
    let Some(host) = registry.host(id) else {
        return;
    };
    let document = host.document();
    source_roots
        .roots
        .retain(|_, root| root.origin_document != Some(id));
    let owner = match lunco_workspace::PinnedDocumentRuntimeOwner::for_document(
        id,
        workspace.map(|workspace| &workspace.0),
    ) {
        Ok(pin) => match pin.runtime {
            lunco_workspace::DocumentRuntimeOwner::Application => SourceRootOwner::Application,
            lunco_workspace::DocumentRuntimeOwner::LocalTwin(twin) => SourceRootOwner::Twin(twin),
            lunco_workspace::DocumentRuntimeOwner::Replicated(owner) => {
                SourceRootOwner::Replicated(owner)
            }
        },
        Err(error) => {
            bevy::log::warn!("cannot admit document source roots: {error}");
            return;
        }
    };
    let path = match document.origin() {
        lunco_doc::DocumentOrigin::File { path, .. } => Some(path.clone()),
        _ => None,
    };
    if path.as_ref().is_some_and(|path| {
        lunco_assets_runtime::library::global_library_sources()
            .iter()
            .any(|source| source.contains(path))
    }) {
        return;
    }
    let source_uri = path
        .as_ref()
        .map(|path| lunco_assets_path::slashed(path))
        .unwrap_or_default();
    let parsed = (!document.syntax_is_stale())
        .then(|| document.strict_ast())
        .flatten()
        .map(|ast| lunco_modelica_ast::resource_uri::SourceFileFacts::parsed(&ast, &source_uri));
    let invalid_facts = lunco_modelica_ast::resource_uri::SourceFileFacts {
        source_uri: source_uri.clone(),
        packages: Vec::new(),
        classes: Vec::new(),
        error: Some(format!("source `{source_uri}` has no valid current parse")),
    };
    // A view contributes current language facts to its existing directory
    // owner. Closing it drops only the unsaved overlay, never the Twin root.
    let mut represented = false;
    for root in source_roots.roots.values_mut() {
        if !path.as_ref().is_some_and(|path| root.origin_document.is_none() && matches!(&root.kind, SourceRootKind::Disk { root_dir } if path.starts_with(root_dir))) {
            root.document_sources.remove(&id);
            continue;
        }
        represented = true;
        let facts = parsed.clone().unwrap_or_else(|| {
            let mut facts = root
                .document_sources
                .get(&id)
                .or_else(|| {
                    root.source_files
                        .iter()
                        .find(|file| file.source_uri == source_uri)
                })
                .cloned()
                .unwrap_or_else(|| invalid_facts.clone());
            facts.error = invalid_facts.error.clone();
            facts
        });
        root.refresh_document_source(id, facts, !document.is_dirty());
    }
    if represented {
        return;
    }
    for class in document.index().classes.values() {
        if rumoca_core::qualified_name_segments(&class.name).count() == 1 {
            let registration = source_roots.register_open_doc_root(
                class.name.clone(),
                path.clone(),
                id,
                owner.clone(),
            );
            if let Some(entry) = registration.and_then(|id| source_roots.roots.get_mut(&id)) {
                if entry.origin_document == Some(id) {
                    entry.source_files =
                        vec![parsed.clone().unwrap_or_else(|| invalid_facts.clone())];
                }
            }
        }
    }
}

/// Source tag for progress entries during source-root loads.
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
    mut pending_unloads: ResMut<PendingSourceRootUnloads>,
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
    let id_prefix = format!("twin:{}:{}:modelica:", request.twin_id, request.name);
    if !request.id.starts_with(&id_prefix) || request.id.trim() == id_prefix {
        return Err(format!(
            "Modelica source-root id `{}` must be scoped to Twin {} authority `{}`",
            request.id, request.twin_id, request.name
        ));
    }
    let authority_root = roots
        .as_deref()
        .ok_or_else(|| "TwinRoots is not installed".to_owned())?
        .root_for(&request.name)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Twin asset authority `{}` is unavailable", request.name))?;
    if authority_root != twin.root {
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
    flush_source_root_unloads(&mut pending_unloads, |command| {
        channels.tx.send(command).is_ok()
    })
    .map_err(|error| format!("cannot retire previous Twin Modelica roots: {error}"))?;
    let mut registry =
        registry.ok_or_else(|| "Modelica source-root registry is not installed".to_owned())?;
    if let Some(existing) = registry.roots.get(&request.id) {
        if existing.owner != SourceRootOwner::Twin(twin_id) {
            return Err(format!(
                "Modelica source-root id `{}` is owned by another runtime scope",
                request.id
            ));
        }
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
        registry.register_twin_disk_root(request.id.clone(), root_dir, twin_id);
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
    let Some(state) = registry.roots.get(id).map(|entry| entry.state.clone()) else {
        return false;
    };
    match &state {
        LoadState::Ready => return true,
        LoadState::Loading { .. } => return false,
        LoadState::Failed(_) => return false,
        LoadState::NotLoaded => {}
    }
    let operation_id = match registry.allocate_operation_id() {
        Ok(id) => id,
        Err(error) => {
            if let Some(entry) = registry.roots.get_mut(id) {
                entry.state = LoadState::Failed(error.clone());
            }
            bevy::log::error!("[source-roots] {error}");
            return false;
        }
    };
    let Some(entry) = registry.roots.get_mut(id) else {
        return false;
    };
    // Native workers resolve/read source files during immutable preparation.
    // The browser keeps its storage reads on the host that owns WebStorage and
    // sends the resulting text to the Modelica Web Worker.
    let (payload, summary) = match &entry.kind {
        SourceRootKind::SourceLibrary => (
            LoadSourceRootPayload::SourceLibrary,
            "installed paired source library".to_owned(),
        ),
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
        operation_id,
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
    entry.operation_id = operation_id;
    // Status-bar feedback is projected from this `Loading` state by the reactive
    // UI observer `ui::core_observers::mirror_source_roots_to_status_bus`. Core
    // sets the state; it no longer touches the status bus.
    true
}

/// Retire every source root owned by the closed Twin. The worker receives the
/// unload in the same FIFO command stream as loads, and operation identities
/// reject any older source preparation that completes afterward.
fn unload_twin_modelica_source_roots(
    trigger: On<lunco_workspace::TwinClosed>,
    channels: Option<Res<ModelicaChannels>>,
    mut registry: Option<ResMut<SourceRootRegistry>>,
    mut pending_unloads: ResMut<PendingSourceRootUnloads>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
) {
    let Some(mut registry) = registry.take() else {
        return;
    };
    let twin = trigger.event().twin;
    let roots = match registry.take_twin_roots(twin) {
        Ok(roots) => roots,
        Err(error) => {
            bevy::log::error!(
                "[source-roots] cannot allocate Twin {} unloads: {error}",
                twin.raw()
            );
            return;
        }
    };
    for (id, operation_id) in roots {
        pending_unloads.queued.push_back((id, operation_id));
    }
    if let Some(channels) = channels {
        if let Err(error) = flush_source_root_unloads(&mut pending_unloads, |command| {
            channels.tx.send(command).is_ok()
        }) {
            report_source_root_unload_dispatch_failure(faults.as_deref_mut(), error);
        }
    }
}

fn flush_pending_source_root_unloads(
    channels: Option<Res<ModelicaChannels>>,
    mut pending_unloads: ResMut<PendingSourceRootUnloads>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
) {
    if pending_unloads.terminal_error.is_some() {
        return;
    }
    if let Some(channels) = channels {
        if let Err(error) = flush_source_root_unloads(&mut pending_unloads, |command| {
            channels.tx.send(command).is_ok()
        }) {
            report_source_root_unload_dispatch_failure(faults.as_deref_mut(), error);
        }
    }
}

fn flush_source_root_unloads(
    pending_unloads: &mut PendingSourceRootUnloads,
    mut send: impl FnMut(ModelicaCommand) -> bool,
) -> Result<(), String> {
    if let Some(error) = &pending_unloads.terminal_error {
        return Err(error.clone());
    }
    while let Some((id, operation_id)) = pending_unloads.queued.front().cloned() {
        if !send(ModelicaCommand::UnloadSourceRoot {
            id: id.clone(),
            operation_id,
        }) {
            let error = format!("cannot unload `{id}`: Modelica worker channel is closed");
            pending_unloads.terminal_error = Some(error.clone());
            bevy::log::error!("[source-roots] {error}");
            return Err(error);
        }
        pending_unloads.queued.pop_front();
        bevy::log::info!("[source-roots] queued unload for `{id}` operation={operation_id}");
    }
    Ok(())
}

fn report_source_root_unload_dispatch_failure(
    faults: Option<&mut lunco_core::RuntimeFaults>,
    error: String,
) {
    if let Some(faults) = faults {
        faults.raise(
            "modelica-source-root-unload-dispatch-failed",
            None,
            "Modelica source-root registry",
            error,
        );
    }
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
    let roots = roots
        .into_iter()
        .map(|namespace| registry.resolve_source_root(&namespace))
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
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

/// Classify Rumoca’s source dependency facts against the registry and log
/// their admission state.
pub fn log_compile_deps(registry: &SourceRootRegistry, model_name: &str, ast: &StoredDefinition) {
    let deps = rumoca_compile::compile::source_root_dependencies(ast);
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
        match registry
            .resolve_source_root(root)
            .ok()
            .as_deref()
            .and_then(|id| registry.state(id))
        {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn twin_root(id: &str, twin: lunco_workspace::TwinId, operation_id: u64) -> SourceRoot {
        SourceRoot {
            id: id.to_owned(),
            namespaces: Vec::new(),
            source_files: Vec::new(),
            document_sources: HashMap::new(),
            pending_saved_sources: HashMap::new(),
            origin_document: None,
            kind: SourceRootKind::Disk {
                root_dir: PathBuf::from("/inline/test/root"),
            },
            owner: SourceRootOwner::Twin(twin),
            operation_id,
            state: LoadState::Loading {
                progress: 0.0,
                started: Instant::now(),
            },
        }
    }

    #[test]
    fn twin_retirement_removes_only_owned_roots_and_advances_operations() {
        let first = lunco_workspace::TwinId::new(17);
        let second = lunco_workspace::TwinId::new(23);
        let mut registry = SourceRootRegistry::default();
        registry.next_operation_id = 3;
        registry.roots.insert(
            "application:control".into(),
            SourceRoot {
                id: "application:control".into(),
                namespaces: Vec::new(),
                source_files: Vec::new(),
                document_sources: HashMap::new(),
                pending_saved_sources: HashMap::new(),
                origin_document: None,
                kind: SourceRootKind::Disk {
                    root_dir: PathBuf::from("/inline/application"),
                },
                owner: SourceRootOwner::Application,
                operation_id: 2,
                state: LoadState::Ready,
            },
        );
        registry.roots.insert(
            "twin:17:lesson:modelica:0".into(),
            twin_root("twin:17:lesson:modelica:0", first, 1),
        );
        registry.roots.insert(
            "twin:23:lesson:modelica:0".into(),
            twin_root("twin:23:lesson:modelica:0", second, 2),
        );

        let retired = registry
            .take_twin_roots(first)
            .expect("Twin root unload operations should be allocated");

        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0].0, "twin:17:lesson:modelica:0");
        assert!(retired[0].1 > 1);
        assert!(registry.roots.contains_key("application:control"));
        assert!(registry.roots.contains_key("twin:23:lesson:modelica:0"));
        assert!(!registry.roots.contains_key("twin:17:lesson:modelica:0"));
    }

    #[test]
    fn parsed_namespaces_follow_exact_source_set_admission_and_retirement() {
        let twin = lunco_workspace::TwinId::new(17);
        let first = "twin:17:lesson:modelica:first";
        let second = "twin:17:lesson:modelica:second";
        let mut registry = SourceRootRegistry::default();
        registry.next_operation_id = 3;
        registry
            .roots
            .insert(first.into(), twin_root(first, twin, 1));
        assert!(registry.resolve_source_root("Shared").is_err());
        assert!(!registry.complete_load(first, 0, vec!["Stale".into()], Vec::new(), None));
        assert!(registry.resolve_source_root("Stale").is_err());
        assert!(registry.complete_load(first, 1, vec!["Shared".into()], Vec::new(), None));
        assert_eq!(registry.resolve_source_root("Shared").unwrap(), first);
        assert!(matches!(registry.state(first), Some(LoadState::Ready)));

        registry
            .roots
            .insert(second.into(), twin_root(second, twin, 2));
        assert!(registry.complete_load(second, 2, vec!["Shared".into()], Vec::new(), None));
        assert!(
            registry
                .resolve_source_root("Shared")
                .unwrap_err()
                .contains("multiple registered")
        );
        registry.take_twin_roots(twin).unwrap();
        assert!(registry.resolve_source_root("Shared").is_err());
        assert!(!registry.complete_load(first, 1, vec!["Shared".into()], Vec::new(), None));
    }

    #[test]
    fn missing_namespace_identity_cannot_publish_ready() {
        let id = "twin:17:lesson:modelica:first";
        let mut registry = SourceRootRegistry::default();
        registry.roots.insert(
            id.into(),
            twin_root(id, lunco_workspace::TwinId::new(17), 1),
        );
        assert!(registry.complete_load(id, 1, Vec::new(), Vec::new(), None));
        assert!(matches!(registry.state(id), Some(LoadState::Failed(error))
            if error.contains("namespace identities")));
    }

    #[test]
    fn pending_unloads_preserve_order_until_the_worker_accepts_them() {
        let mut pending = PendingSourceRootUnloads::default();
        pending
            .queued
            .push_back(("twin:17:lesson:modelica:0".into(), 8));
        pending
            .queued
            .push_back(("twin:17:lesson:modelica:1".into(), 9));

        let mut sent = Vec::new();
        flush_source_root_unloads(&mut pending, |command| {
            let ModelicaCommand::UnloadSourceRoot { id, operation_id } = command else {
                panic!("pending source-root operation must be an unload");
            };
            sent.push((id, operation_id));
            true
        })
        .expect("the worker accepts both unloads");
        assert!(pending.queued.is_empty());
        assert_eq!(
            sent,
            vec![
                ("twin:17:lesson:modelica:0".into(), 8),
                ("twin:17:lesson:modelica:1".into(), 9),
            ]
        );
    }

    #[test]
    fn failed_unload_dispatch_is_terminal_without_retrying_each_update() {
        let mut pending = PendingSourceRootUnloads::default();
        pending
            .queued
            .push_back(("twin:17:lesson:modelica:0".into(), 8));
        let attempts = std::cell::Cell::new(0);

        let first = flush_source_root_unloads(&mut pending, |_| {
            attempts.set(attempts.get() + 1);
            false
        })
        .expect_err("a closed worker channel rejects the unload");
        assert!(first.contains("worker channel is closed"));
        assert_eq!(pending.queued.len(), 1);

        let second = flush_source_root_unloads(&mut pending, |_| {
            attempts.set(attempts.get() + 1);
            true
        })
        .expect_err("terminal dispatch failure remains visible");
        assert_eq!(second, first);
        assert_eq!(attempts.get(), 1, "a terminal failure is not retried");
    }
}

pub mod resources;
