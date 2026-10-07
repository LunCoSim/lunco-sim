//! **Scene Files** — every file the loaded scene is actually made of.
//!
//! # Why this is not the Files section
//!
//! [`FilesSection`](lunco_workbench_browser::FilesSection) shows a FOLDER (the active
//! Twin's tree) and [`UsdSceneSection`](crate::browser_section::UsdSceneSection)
//! shows the open STAGES. Neither answers "what is this scene composed of", which
//! is a graph question: a scene pulls its rovers from `assets/vessels/…`, those
//! pull components from `assets/components/…`, and those bind `.mo` models,
//! `.rhai` policies, shaders and textures by attribute. None of that is in the
//! scene's folder, and most of it is in no open tab.
//!
//! # The `lunco://` hole this had to close first
//!
//! Shipped assets are REQUIRED to be referenced as `@lunco://…@`, and
//! [`lunco_assets_core::transitive_file_closure`] drops every schemed arc because it
//! has no resolver — so the plain walk reports a library-built scene as one
//! file. This section uses [`lunco_assets_core::transitive_file_closure_with`] and
//! supplies the resolver: `lunco://` against the shipped asset root, `twin://`
//! against an immutable [`TwinRootsSnapshot`]. USD dependency interpretation comes from
//! `lunco-usd-compose`; asset traversal and storage stay in `lunco-assets`.
//! Anything it still cannot reach is COUNTED and shown, so a partial answer
//! never reads as a complete one.
//!
//! # The Modelica schemes come for free
//!
//! A `.mo` row opens a model tab, and the diagram is projected from the class
//! whether or not the source carries `annotation(Placement(…))` — un-annotated
//! components are laid out on a grid by
//! `DiagramAutoLayoutSettings`. So "show the scene's auto-generated Modelica
//! schemes" needs no diagram work here: it needs the `.mo` files to be REACHABLE
//! and clickable, which is exactly what the resolved closure provides.
//!
//! # Cost
//!
//! One bounded native worker prepares an immutable document/mount snapshot when
//! its inputs change or the user requests refresh. The latest request coalesces
//! behind it. Publication checks exact owners, generations and mount revision;
//! paint reads only the published rows. Scope retirement clears the old view.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use egui;
use lunco_assets_core::{FileClosureLimits, TwinRootsSnapshot};
use lunco_doc::DocumentOrigin;
use lunco_doc_bevy::{DocumentRegistry, OpenFile};
use lunco_workbench_browser::{
    BrowserAction, BrowserCtx, BrowserQuery, BrowserScope, BrowserSection,
};

use lunco_usd_core::commands::is_usd_path;
use lunco_usd_document::document::UsdDocument;

/// What kind of file a row is — decides its group and its click action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SceneFileKind {
    /// A USD layer (`.usda`/`.usd`/`.usdc`) — the scene itself and everything it
    /// references. Other package formats remain assets until their document
    /// loader is supported by the USD domain.
    Layer,
    /// A Modelica model bound by `info:sourceAsset`. Opens as a diagram.
    Modelica,
    /// A `.rhai` scenario / policy.
    Script,
    /// A WGSL shader.
    Shader,
    /// Anything else the closure carried: meshes, textures, DEMs.
    Asset,
}

impl SceneFileKind {
    fn of(path: &Path) -> Self {
        if is_usd_path(&path.to_string_lossy()) {
            return Self::Layer;
        }
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("mo") => Self::Modelica,
            Some("rhai") => Self::Script,
            Some("wgsl") => Self::Shader,
            _ => Self::Asset,
        }
    }

    /// Group header, in render order.
    pub fn title(self) -> &'static str {
        match self {
            Self::Layer => "USD layers",
            Self::Modelica => "Modelica models",
            Self::Script => "Scripts",
            Self::Shader => "Shaders",
            Self::Asset => "Assets",
        }
    }

    /// Every kind, in the order the section paints them: the structure first,
    /// then behaviour, then the leaves.
    pub const ORDER: [SceneFileKind; 5] = [
        Self::Layer,
        Self::Modelica,
        Self::Script,
        Self::Shader,
        Self::Asset,
    ];

    /// True for source files owned by the shared text viewer rather than a
    /// typed USD or Modelica document editor.
    fn opens_in_source_view(self) -> bool {
        matches!(self, Self::Script | Self::Shader)
    }
}

/// One file in the scene's closure.
#[derive(Debug, Clone)]
pub struct SceneFileRow {
    /// Absolute path on disk.
    pub path: PathBuf,
    /// Display label — the path relative to its library/twin root when it has
    /// one, so rows read `vessels/rovers/rocker_bogie.usda` rather than a wall of
    /// absolute prefix.
    pub label: String,
    /// Which group it renders under.
    pub kind: SceneFileKind,
    /// The file the closure named does not exist on disk. Shown rather than
    /// hidden: a dangling reference is precisely what someone opening this
    /// section is hunting for.
    pub missing: bool,
}

/// Change-gated view-model of the loaded scene's file closure. Produced by
/// [`produce_scene_file_view`]; read (never built) by [`SceneFilesSection`].
#[derive(Resource, Default)]
pub struct SceneFileView {
    /// The scene roots the closure was walked from.
    pub roots: Vec<PathBuf>,
    /// Every reachable file, grouped by [`SceneFileKind`] at paint time.
    pub rows: Vec<SceneFileRow>,
    /// References that named a scheme this session cannot resolve (a `twin://`
    /// with no such Twin mounted). Reported as a count so a partial listing is
    /// never mistaken for a complete one.
    pub unresolved: usize,
    pub preparing: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SceneFileDocument {
    owner: lunco_workspace::PinnedDocumentRuntimeOwner,
    generation: u64,
    path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SceneFileInputs {
    documents: Vec<SceneFileDocument>,
    scope: lunco_workspace::DocumentRuntimeOwner,
    replication: Option<lunco_workspace::ReplicationOwner>,
    mount_revision: Option<u64>,
    limits: FileClosureLimits,
}
impl SceneFileInputs {
    fn roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<_> = self
            .documents
            .iter()
            .map(|document| document.path.clone())
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }
    fn same_scope(&self, other: &Self) -> bool {
        self.scope == other.scope
            && self.replication == other.replication
            && self.mount_revision == other.mount_revision
    }
}

struct SceneFileTask {
    inputs: SceneFileInputs,
    operation: u64,
    task: bevy::tasks::Task<(
        lunco_core_runtime::async_work::ExternalWorkPermit,
        Result<SceneFileView, String>,
    )>,
}

/// Sole owner of one preparation and one coalesced latest request.
#[derive(Resource, Default)]
pub struct SceneFilePreparation {
    requested: Option<SceneFileInputs>,
    desired: Option<(SceneFileInputs, u64)>,
    pending: Option<SceneFileTask>,
    operation: u64,
    /// Admission capacity revision observed when the bounded queue was full.
    /// The desired request waits for the next capacity change instead of
    /// failing: a full queue is backpressure, not a scene-file error.
    deferred_at_capacity: Option<u64>,
}

/// Set by the section's ↻ button to force one rebuild — the roots did not change,
/// but the files on disk may have.
#[derive(Resource, Default)]
pub struct SceneFileRescan(pub bool);

/// Resolve a schemed reference to a file. `lunco://` re-roots on the shipped
/// asset library, `twin://` on the named Twin's root; anything else (a leading
/// `/`, an unknown scheme) is unreachable and counted by the caller.
/// `twins` is optional for the same reason the system's `Res` is: a host with no
/// Twin source mounted can still resolve the shipped library, and `twin://` there
/// is simply unreachable (counted, not fatal).
fn resolve_scheme(
    reference: &str,
    assets_root: Option<&Path>,
    twins: Option<&TwinRootsSnapshot>,
) -> Option<PathBuf> {
    if let Some(rel) = lunco_assets_core::parse_lunco_uri(reference) {
        if !lunco_assets_path::is_safe_relative_path(rel) {
            return None;
        }
        return Some(assets_root?.join(rel));
    }
    if let Some((name, rel)) = lunco_assets_core::parse_twin_uri(reference) {
        let relative = lunco_assets_path::relative_path(rel)?;
        return match twins?.resolve_file(name, &relative) {
            Ok(path) => path,
            Err(error) => {
                error!("[scene-files] Twin asset lookup failed for `{reference}`: {error}");
                None
            }
        };
    }
    None
}

/// The shipped asset root to resolve `lunco://` against: the `assets/` ancestor
/// of a scene root when the scene lives inside a library tree, else the running
/// project's own `assets/`. Both are what the `lunco://` asset SOURCE would use,
/// which is the point — the browser must list the files the engine would load.
fn assets_root_for(roots: &[PathBuf]) -> Option<PathBuf> {
    for r in roots {
        if let Some(found) = lunco_assets_core::shipped_asset_root(r) {
            return Some(found.to_path_buf());
        }
    }
    match lunco_assets_core::assets_dir_abs() {
        Ok(root) => Some(root),
        Err(error) => {
            bevy::log::error!("scene browser asset root failed: {error}");
            None
        }
    }
}

/// Label a path relative to whichever root it sits under, so rows stay readable.
fn label_for(path: &Path, assets_root: Option<&Path>, roots: &[PathBuf]) -> String {
    if let Some(rel) = assets_root.and_then(|a| path.strip_prefix(a).ok()) {
        return rel.to_string_lossy().into_owned();
    }
    for r in roots {
        if let Some(dir) = r.parent() {
            if let Ok(rel) = path.strip_prefix(dir) {
                return rel.to_string_lossy().into_owned();
            }
        }
    }
    path.to_string_lossy().into_owned()
}

/// Snapshot current scoped document roots without filesystem reads.
fn current_scene_file_documents(world: &World) -> Vec<SceneFileDocument> {
    let Some(registry) = world.get_resource::<DocumentRegistry<UsdDocument>>() else {
        return Vec::new();
    };
    let workspace = world
        .get_resource::<lunco_workspace::WorkspaceResource>()
        .map(|workspace| &workspace.0);
    let replication = lunco_core_session::current_replication_owner_in(world);
    let mut documents: Vec<_> = registry
        .ids()
        .filter_map(|id| {
            let owner =
                lunco_workspace::PinnedDocumentRuntimeOwner::for_document(id, workspace).ok()?;
            if !owner.is_in_active_scope(workspace, replication.as_ref()) {
                return None;
            }
            let host = registry.host(id)?;
            match host.document().origin() {
                DocumentOrigin::File { path, .. } => Some(SceneFileDocument {
                    owner,
                    generation: host.generation(),
                    path: path.clone(),
                }),
                _ => None,
            }
        })
        .collect();
    documents.sort_by_key(|document| document.owner.document.raw());
    documents
}

/// Native worker preparation of the resolved reference closure and row metadata.
/// Only immutable input snapshots cross this boundary.
fn prepare_scene_file_view(
    inputs: &SceneFileInputs,
    twins: Option<&TwinRootsSnapshot>,
) -> Result<SceneFileView, String> {
    let roots = inputs.roots();
    let assets_root = assets_root_for(&roots);
    let unresolved = std::sync::atomic::AtomicUsize::new(0);
    let files = lunco_assets_core::transitive_file_closure_with(
        &roots,
        &inputs.limits,
        |reference| {
            let resolved = resolve_scheme(reference, assets_root.as_deref(), twins);
            if resolved.is_none() {
                unresolved.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            resolved
        },
        lunco_usd_compose::is_usd_layer,
        lunco_usd_compose::layer_dependency_arcs,
    )
    .map_err(|error| error.to_string())?;

    use lunco_storage::Storage;
    let storage = lunco_storage::FileStorage::new();
    let mut rows: Vec<SceneFileRow> = files
        .into_iter()
        .map(|path| {
            let missing =
                match storage.entry_kind_sync(&lunco_storage::StorageHandle::File(path.clone())) {
                    Ok(_) => false,
                    Err(lunco_storage::StorageError::NotFound) => true,
                    Err(error) => {
                        return Err(format!(
                            "cannot inspect scene asset {}: {error}",
                            path.display()
                        ));
                    }
                };
            Ok(SceneFileRow {
                label: label_for(&path, assets_root.as_deref(), &roots),
                kind: SceneFileKind::of(&path),
                missing,
                path,
            })
        })
        .collect::<Result<_, String>>()?;
    rows.sort_by(|a, b| (a.kind, &a.label).cmp(&(b.kind, &b.label)));

    Ok(SceneFileView {
        roots,
        rows,
        unresolved: unresolved.into_inner(),
        preparing: false,
        error: None,
    })
}

fn capture_scene_file_inputs(world: &World) -> Result<SceneFileInputs, String> {
    let replication = lunco_core_session::current_replication_owner_in(world);
    let scope = match replication.as_ref() {
        Some(owner @ lunco_workspace::ReplicationOwner::Twin { .. }) => {
            lunco_workspace::DocumentRuntimeOwner::Replicated(owner.clone())
        }
        _ => world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|workspace| workspace.active_twin)
            .map_or(
                lunco_workspace::DocumentRuntimeOwner::Application,
                lunco_workspace::DocumentRuntimeOwner::LocalTwin,
            ),
    };
    let mount_revision = world
        .get_resource::<lunco_assets_core::TwinRoots>()
        .map(|roots| roots.revision())
        .transpose()
        .map_err(|error| error.to_string())?;
    let limits = *world
        .get_resource::<FileClosureLimits>()
        .ok_or("scene file traversal limits are unavailable")?;
    limits.validate().map_err(|error| error.to_string())?;
    Ok(SceneFileInputs {
        documents: current_scene_file_documents(world),
        scope,
        replication,
        mount_revision,
        limits,
    })
}

fn scene_file_preparation_failed(world: &mut World, message: String) {
    let mut view = world.resource_mut::<SceneFileView>();
    let changed = view.error.as_ref() != Some(&message);
    view.preparing = false;
    view.error = Some(message.clone());
    if changed {
        world.trigger(lunco_core::RuntimeError {
            name: "scene-file-preparation-failed".into(),
            message,
        });
    }
}

fn accepts_scene_file_result(
    captured: &SceneFileInputs,
    operation: u64,
    current: &SceneFileInputs,
    latest: u64,
) -> bool {
    captured == current && operation == latest
}

/// Poll one bounded native preparation; the latest immutable request wins only
/// after its exact source pins, generations, scope and mount revision agree.
pub fn produce_scene_file_view(world: &mut World) {
    let forced = std::mem::replace(&mut world.resource_mut::<SceneFileRescan>().0, false);
    let current = capture_scene_file_inputs(world);
    world.resource_scope(|world, mut state: Mut<SceneFilePreparation>| {
        let inputs = match current {
            Ok(inputs) => inputs,
            Err(error) => {
                state.requested = None;
                state.desired = None;
                if let Some(pending) = state.pending.as_mut() {
                    if bevy::tasks::futures_lite::future::block_on(
                        bevy::tasks::futures_lite::future::poll_once(&mut pending.task),
                    )
                    .is_some()
                    {
                        state.pending = None;
                    }
                }
                {
                    let mut view = world.resource_mut::<SceneFileView>();
                    view.roots.clear();
                    view.rows.clear();
                    view.unresolved = 0;
                }
                scene_file_preparation_failed(world, error);
                return;
            }
        };
        if forced || state.requested.as_ref() != Some(&inputs) {
            let same_scope = state
                .requested
                .as_ref()
                .is_some_and(|previous| previous.same_scope(&inputs));
            if !same_scope || inputs.documents.is_empty() {
                *world.resource_mut::<SceneFileView>() = SceneFileView::default();
            }
            world.resource_mut::<SceneFileView>().roots = inputs.roots();
            let Some(operation) = state.operation.checked_add(1) else {
                scene_file_preparation_failed(
                    world,
                    "scene file request sequence exhausted".into(),
                );
                return;
            };
            state.operation = operation;
            state.requested = Some(inputs.clone());
            state.desired = (!inputs.documents.is_empty()).then(|| (inputs.clone(), operation));
            world.resource_mut::<SceneFileView>().preparing = state.desired.is_some();
        }
        if let Some(pending) = state.pending.as_mut() {
            let result = bevy::tasks::futures_lite::future::block_on(
                bevy::tasks::futures_lite::future::poll_once(&mut pending.task),
            );
            if let Some((_permit, result)) = result {
                let pending = state.pending.take().expect("polled pending task");
                if accepts_scene_file_result(
                    &pending.inputs,
                    pending.operation,
                    &inputs,
                    state.operation,
                ) {
                    match result {
                        Ok(view) => *world.resource_mut::<SceneFileView>() = view,
                        Err(error) => scene_file_preparation_failed(world, error),
                    }
                }
            }
        }
        if state.pending.is_some() {
            return;
        }
        let Some((desired, operation)) = state.desired.take() else {
            return;
        };
        let capacity = world
            .get_resource::<lunco_core_runtime::AsyncWorkAdmission>()
            .map(lunco_core_runtime::AsyncWorkAdmission::capacity_revision);
        if state.deferred_at_capacity.is_some() && state.deferred_at_capacity == capacity {
            state.desired = Some((desired, operation));
            return;
        }
        let admitted = (|| {
            let mounts = world
                .get_resource::<lunco_assets_core::TwinRoots>()
                .map(|roots| roots.snapshot())
                .transpose()
                .map_err(|error| error.to_string())?;
            if mounts.as_ref().map(|mounts| mounts.revision) != desired.mount_revision {
                return Err(
                    "Twin asset registry changed before scene file preparation admission"
                        .to_owned(),
                );
            }
            let pool = bevy::tasks::IoTaskPool::try_get()
                .ok_or("scene file I/O task pool is unavailable")?;
            let admission = world
                .get_resource::<lunco_core_runtime::AsyncWorkAdmission>()
                .ok_or("bounded async work admission is unavailable")?;
            let scope_generation = match &desired.scope {
                lunco_workspace::DocumentRuntimeOwner::Application => 0,
                lunco_workspace::DocumentRuntimeOwner::LocalTwin(twin) => twin.raw(),
                lunco_workspace::DocumentRuntimeOwner::Replicated(owner) => match owner.scope() {
                    lunco_workspace::ReplicationScope::Application => 0,
                    lunco_workspace::ReplicationScope::Twin(twin) => twin.raw(),
                },
            };
            let permit = match admission.admit_external(
                lunco_core_runtime::AsyncWorkPriority::Interactive,
                lunco_core_runtime::AsyncWorkKey::new(
                    lunco_core_runtime::AsyncWorkKind::VisualizationPreparation,
                    scope_generation,
                    0x7363656e652d66696c6573,
                    desired.mount_revision.unwrap_or(0),
                    operation,
                ),
            ) {
                Ok(permit) => permit,
                Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
                    return Ok(None);
                }
                Err(error) => {
                    return Err(format!(
                        "scene file preparation admission rejected: {error:?}"
                    ));
                }
            };
            let inputs = desired.clone();
            Ok(Some(SceneFileTask {
                inputs: desired.clone(),
                operation,
                task: pool.spawn(async move {
                    let result = prepare_scene_file_view(&inputs, mounts.as_ref());
                    (permit, result)
                }),
            }))
        })();
        match admitted {
            Ok(Some(pending)) => {
                state.deferred_at_capacity = None;
                state.pending = Some(pending);
            }
            Ok(None) => {
                state.deferred_at_capacity = capacity;
                state.desired = Some((desired, operation));
            }
            Err(error) => scene_file_preparation_failed(world, error),
        }
    });
}

/// Browser section listing the loaded scene's file closure.
///
/// Files scope: it answers "what is on disk behind this scene", the same question
/// the Files tree answers for a folder. The Models tab keeps the typed views
/// (stages, Modelica classes).
#[derive(Default)]
pub struct SceneFilesSection;

impl BrowserSection for SceneFilesSection {
    fn id(&self) -> &str {
        "lunco.usd.scene-files"
    }

    fn title(&self) -> &str {
        "Scene"
    }

    fn scope(&self) -> BrowserScope {
        BrowserScope::Files
    }

    fn default_open(&self) -> bool {
        true
    }

    fn order(&self) -> u32 {
        // Above the raw folder tree (200): what the scene IS comes before what
        // happens to sit next to it on disk.
        150
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_, '_>) {
        let Some(view) = ctx.resource::<SceneFileView>() else {
            ui.label(
                egui::RichText::new("SceneFileView resource missing")
                    .weak()
                    .italics(),
            );
            return;
        };
        // Snapshot before any `&mut` dispatch below.
        let query = ctx.resource::<BrowserQuery>().cloned().unwrap_or_default();
        let rows: Vec<SceneFileRow> = view
            .rows
            .iter()
            .filter(|row| {
                !query.is_active()
                    || query.matches(&row.label)
                    || query.matches(&row.path.to_string_lossy())
                    || query.matches(row.kind.title())
            })
            .cloned()
            .collect();
        let unresolved = view.unresolved;
        let no_roots = view.roots.is_empty();
        if let Some(error) = &view.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if view.preparing {
            ui.label("Preparing scene files…");
        }

        if lunco_workbench_widgets::icon_button(
            ui,
            lunco_workbench_widgets::UiIcon::Refresh,
            "Re-walk the scene's references",
        )
        .clicked()
        {
            ctx.set_resource(SceneFileRescan(true));
        }

        if no_roots {
            ui.label(
                egui::RichText::new("No file-backed scene open.")
                    .weak()
                    .italics(),
            );
            return;
        }

        if rows.is_empty() {
            ui.label(
                egui::RichText::new("No scene files match the filter.")
                    .weak()
                    .italics(),
            );
            return;
        }

        let mut clicked: Option<(PathBuf, SceneFileKind)> = None;
        for kind in SceneFileKind::ORDER {
            let group: Vec<&SceneFileRow> = rows.iter().filter(|r| r.kind == kind).collect();
            if group.is_empty() {
                continue;
            }
            egui::CollapsingHeader::new(format!("{} ({})", kind.title(), group.len()))
                .id_salt(("scene_files_group", kind.title()))
                .default_open(matches!(
                    kind,
                    SceneFileKind::Layer | SceneFileKind::Modelica
                ))
                .show(ui, |ui| {
                    for row in group {
                        // Openable rows are routed through the normal typed
                        // `OpenFile` surface. USD and Modelica have richer
                        // editors; Rhai and WGSL intentionally open in the
                        // shared source viewer so the exact behaviour/shader
                        // driving the scene is inspectable from the closure.
                        let openable = !row.missing
                            && (matches!(row.kind, SceneFileKind::Layer | SceneFileKind::Modelica)
                                || row.kind.opens_in_source_view());
                        let resp = ui
                            .horizontal(|ui| {
                                if row.missing {
                                    let (icon_rect, _) = ui.allocate_exact_size(
                                        egui::vec2(18.0, 18.0),
                                        egui::Sense::hover(),
                                    );
                                    lunco_workbench_widgets::paint_icon(
                                        ui.painter(),
                                        lunco_workbench_widgets::UiIcon::Warning,
                                        icon_rect,
                                        ui.visuals().error_fg_color,
                                    );
                                }
                                if openable {
                                    ui.selectable_label(false, row.label.clone())
                                } else {
                                    // `sense(hover)` so the tooltip below still fires —
                                    // a bare `Label` senses nothing and would swallow it.
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(row.label.clone()).weak(),
                                        )
                                        .sense(egui::Sense::hover()),
                                    )
                                }
                            })
                            .inner;
                        let resp = if row.missing {
                            resp.on_hover_text("Referenced by the scene but not on disk")
                        } else if openable {
                            resp.on_hover_text(row.path.to_string_lossy())
                        } else {
                            resp.on_hover_text(format!(
                                "{} — part of the scene; no editor is bound to this type",
                                row.path.to_string_lossy()
                            ))
                        };
                        if openable && resp.clicked() {
                            clicked = Some((row.path.clone(), row.kind));
                        }
                    }
                });
        }

        if unresolved > 0 {
            ui.label(
                egui::RichText::new(format!(
                    "{unresolved} reference(s) could not be resolved (unmounted Twin or unknown \
                     scheme) — the list is partial."
                ))
                .weak(),
            );
        }

        if let Some((path, kind)) = clicked {
            if kind.opens_in_source_view() {
                // Scripts and shaders are source text, not USD/Modelica
                // documents. Send the typed source command directly so the
                // shared text viewer owns the absolute scene-closure path;
                // BrowserAction is reserved for domain document dispatchers.
                ctx.trigger(OpenFile {
                    path: path.to_string_lossy().into_owned(),
                });
            } else {
                // Absolute path: the domain dispatchers (`.mo` → Modelica model
                // tab, `.usda` → USD) take it as-is rather than anchoring on the
                // active Twin, because a scene's files routinely live outside it.
                ctx.actions.push(BrowserAction::OpenFile {
                    relative_path: path,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_assets_core::TwinRoots;
    use lunco_storage::Storage;

    #[test]
    fn scene_file_publication_rejects_retired_owner_and_superseded_refresh() {
        use lunco_workspace::{DocumentRuntimeOwner, PinnedDocumentRuntimeOwner, TwinId};
        let inputs = SceneFileInputs {
            documents: vec![SceneFileDocument {
                owner: PinnedDocumentRuntimeOwner {
                    document: lunco_doc::DocumentId(1),
                    runtime: DocumentRuntimeOwner::LocalTwin(TwinId::new(1)),
                },
                generation: 1,
                path: PathBuf::from("same-root/scene.node"),
            }],
            scope: DocumentRuntimeOwner::LocalTwin(TwinId::new(1)),
            replication: None,
            mount_revision: Some(1),
            limits: FileClosureLimits::default(),
        };
        assert!(accepts_scene_file_result(&inputs, 1, &inputs, 1));
        assert!(
            !accepts_scene_file_result(&inputs, 1, &inputs, 2),
            "an explicit refresh supersedes the old job"
        );
        let mut reopened = inputs.clone();
        reopened.documents[0].owner.runtime = DocumentRuntimeOwner::LocalTwin(TwinId::new(2));
        reopened.scope = DocumentRuntimeOwner::LocalTwin(TwinId::new(2));
        assert_eq!(reopened.roots(), inputs.roots());
        assert!(!inputs.same_scope(&reopened));
        assert!(!accepts_scene_file_result(&inputs, 1, &reopened, 1));
        let mut changed = inputs.clone();
        changed.documents[0].generation += 1;
        assert!(!accepts_scene_file_result(&inputs, 1, &changed, 1));
        changed = inputs.clone();
        changed.mount_revision = Some(2);
        assert!(!accepts_scene_file_result(&inputs, 1, &changed, 1));
        changed = inputs.clone();
        changed.documents.clear();
        changed.scope = DocumentRuntimeOwner::Application;
        assert!(!accepts_scene_file_result(&inputs, 1, &changed, 1));
    }

    #[test]
    fn scene_file_roots_retire_exact_twin_context_and_restore_application_scope() {
        use lunco_workspace::{
            DocumentEntry, DocumentKindId, DocumentRuntimeOwner, TwinMode, WorkspaceResource,
        };
        let root = tempfile::tempdir().expect("generic folder root");
        let other = tempfile::tempdir().expect("other generic folder root");
        let folder = |path: &Path| match TwinMode::open(path).expect("empty folder admission") {
            TwinMode::Folder(twin) | TwinMode::Twin(twin) => twin,
            TwinMode::Orphan(_) => panic!("temporary root is a folder"),
        };
        let mut world = World::new();
        world.init_resource::<DocumentRegistry<UsdDocument>>();
        world.init_resource::<WorkspaceResource>();
        let application = other.path().join("application.usda");
        let retired = root.path().join("scene.usda");
        let admit = |world: &mut World, path: &Path, runtime: DocumentRuntimeOwner| {
            let document = world
                .resource::<DocumentRegistry<UsdDocument>>()
                .reserve_id();
            let origin = DocumentOrigin::writable_file(path);
            world
                .resource_mut::<DocumentRegistry<UsdDocument>>()
                .install_prebuilt(
                    document,
                    UsdDocument::with_origin(document, "#usda 1.0\n", origin.clone()),
                )
                .expect("generic resident document");
            world
                .resource_mut::<WorkspaceResource>()
                .add_document(DocumentEntry {
                    id: document,
                    kind: DocumentKindId::new("usd"),
                    origin,
                    runtime_context: runtime,
                    title: "Generic document".into(),
                    dirty: false,
                });
            document
        };
        admit(&mut world, &application, DocumentRuntimeOwner::Application);
        world.init_resource::<FileClosureLimits>();
        assert_eq!(
            capture_scene_file_inputs(&world).unwrap().roots(),
            vec![application.clone()]
        );
        let first = world
            .resource_mut::<WorkspaceResource>()
            .add_twin(folder(root.path()));
        let old_document = admit(&mut world, &retired, DocumentRuntimeOwner::LocalTwin(first));
        assert_eq!(
            capture_scene_file_inputs(&world).unwrap().roots(),
            vec![retired.clone()]
        );
        world.resource_mut::<WorkspaceResource>().close_twin(first);
        assert!(
            world
                .resource::<DocumentRegistry<UsdDocument>>()
                .host(old_document)
                .is_some()
        );
        assert!(
            world
                .resource::<WorkspaceResource>()
                .document(old_document)
                .is_some()
        );
        assert_eq!(
            capture_scene_file_inputs(&world).unwrap().roots(),
            vec![application.clone()]
        );
        let reopened = world
            .resource_mut::<WorkspaceResource>()
            .add_twin(folder(root.path()));
        assert_ne!(first, reopened);
        assert!(
            capture_scene_file_inputs(&world)
                .unwrap()
                .roots()
                .is_empty(),
            "retained source cannot acquire a reopened Twin owner"
        );
        admit(
            &mut world,
            &retired,
            DocumentRuntimeOwner::LocalTwin(reopened),
        );
        assert_eq!(
            capture_scene_file_inputs(&world).unwrap().roots(),
            vec![retired.clone()]
        );
        let inactive = world
            .resource_mut::<WorkspaceResource>()
            .add_twin(folder(other.path()));
        admit(
            &mut world,
            &other.path().join("inactive.usda"),
            DocumentRuntimeOwner::LocalTwin(inactive),
        );
        assert_eq!(
            capture_scene_file_inputs(&world).unwrap().roots(),
            vec![retired]
        );
        world
            .resource_mut::<WorkspaceResource>()
            .close_twin(inactive);
        world
            .resource_mut::<WorkspaceResource>()
            .close_twin(reopened);
        assert_eq!(
            capture_scene_file_inputs(&world).unwrap().roots(),
            vec![application]
        );
    }

    #[test]
    fn kinds_route_by_extension() {
        assert_eq!(
            SceneFileKind::of(Path::new("/a/scene.usda")),
            SceneFileKind::Layer
        );
        assert_eq!(
            SceneFileKind::of(Path::new("/a/scene.usd")),
            SceneFileKind::Layer
        );
        assert_eq!(
            SceneFileKind::of(Path::new("/a/package.usdz")),
            SceneFileKind::Asset
        );
        assert_eq!(
            SceneFileKind::of(Path::new("/a/Drive.mo")),
            SceneFileKind::Modelica
        );
        assert_eq!(
            SceneFileKind::of(Path::new("/a/mission.rhai")),
            SceneFileKind::Script
        );
        assert_eq!(
            SceneFileKind::of(Path::new("/a/wheel.wgsl")),
            SceneFileKind::Shader
        );
        assert_eq!(
            SceneFileKind::of(Path::new("/a/rover.glb")),
            SceneFileKind::Asset
        );
    }

    #[test]
    fn scripts_and_shaders_use_the_shared_source_viewer() {
        assert!(SceneFileKind::Script.opens_in_source_view());
        assert!(SceneFileKind::Shader.opens_in_source_view());
        assert!(!SceneFileKind::Layer.opens_in_source_view());
        assert!(!SceneFileKind::Modelica.opens_in_source_view());
    }

    #[test]
    fn lunco_references_resolve_against_the_library_root() {
        let twins = TwinRoots::default();
        let assets = PathBuf::from("/proj/assets");
        assert_eq!(
            resolve_scheme(
                "lunco://vessels/rover.usda",
                Some(&assets),
                Some(&twins.snapshot().unwrap())
            ),
            Some(assets.join("vessels/rover.usda"))
        );
        // The shipped library does not need a Twin source to be mounted.
        assert_eq!(
            resolve_scheme("lunco://vessels/rover.usda", Some(&assets), None),
            Some(assets.join("vessels/rover.usda"))
        );
        assert_eq!(
            resolve_scheme(
                "twin://nope/scene.usda",
                Some(&assets),
                Some(&twins.snapshot().unwrap())
            ),
            None,
            "an unmounted twin is unreachable, not silently mis-rooted"
        );
        assert_eq!(
            resolve_scheme(
                "/absolute/from/source/root.usda",
                Some(&assets),
                Some(&twins.snapshot().unwrap())
            ),
            None
        );
    }

    #[test]
    fn twin_references_resolve_against_the_mounted_root() {
        let root = tempfile::tempdir().expect("temporary Twin root");
        let scene = root.path().join("scenes/base.usda");
        lunco_storage::FileStorage::new()
            .write_sync(
                &lunco_storage::StorageHandle::File(scene.clone()),
                b"#usda 1.0\n",
            )
            .unwrap();
        let twins = TwinRoots::default();
        let name = twins
            .register("moonbase", root.path())
            .expect("register root");
        let uri = format!("twin://{name}/scenes/base.usda");
        assert_eq!(
            resolve_scheme(&uri, None, Some(&twins.snapshot().unwrap())),
            Some(scene)
        );
    }

    #[test]
    fn scheme_references_cannot_escape_their_root() {
        let twins = TwinRoots::default();
        let assets = PathBuf::from("/proj/assets");
        let root = tempfile::tempdir().expect("temporary Twin root");
        let name = twins
            .register("moonbase", root.path())
            .expect("register root");
        for reference in [
            "lunco://../outside.usda",
            "lunco://vessels/../../outside.usda",
            &format!("twin://{name}/../outside.usda"),
            &format!("twin://{name}/scenes/../../outside.usda"),
        ] {
            assert_eq!(
                resolve_scheme(reference, Some(&assets), Some(&twins.snapshot().unwrap())),
                None,
                "unsafe reference must be rejected: {reference}"
            );
        }
    }

    #[test]
    fn labels_are_relative_to_the_library_root() {
        let assets = PathBuf::from("/proj/assets");
        assert_eq!(
            label_for(
                Path::new("/proj/assets/vessels/rovers/rb.usda"),
                Some(&assets),
                &[]
            ),
            "vessels/rovers/rb.usda"
        );
        // Outside the library: relative to the scene's own folder.
        assert_eq!(
            label_for(
                Path::new("/work/scenes/props/rock.usda"),
                Some(&assets),
                &[PathBuf::from("/work/scenes/scene.usda")]
            ),
            "props/rock.usda"
        );
    }
}
