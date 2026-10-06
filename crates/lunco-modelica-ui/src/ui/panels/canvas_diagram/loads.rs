//! Drill-in and duplicate document loaders.
//!
//! Two parallel pipelines: drill-in opens source library classes read-only,
//! duplicate creates an editable Untitled copy. Both reserve a doc
//! id eagerly, schedule preparation on
//! `AsyncComputeTaskPool`, and install the prebuilt
//! [`lunco_modelica_document::ModelicaDocument`] via
//! [`crate::ui::document_context::ModelicaDocuments::install_prebuilt`]
//! when the load completes. The in-flight task and metadata live
//! in [`crate::ui::document_openings::DocumentOpenings`]; the
//! per-frame drivers below poll their own variant. Native tasks run on the
//! task pool; browser scheduling does not imply a separate JavaScript worker.

use crate::ui::document_context::ModelicaDocuments;
use crate::ui::document_openings::{DocumentOpenings, OpeningState};
use bevy::prelude::*;

/// Open a Modelica class selected in a canvas node through the asynchronous
/// domain loader. The panel only carries the qualified class name.
#[derive(Event)]
pub(crate) struct DrillIntoClassRequested {
    pub(crate) qualified: String,
}

pub(crate) fn on_drill_into_class_requested(
    trigger: On<DrillIntoClassRequested>,
    mut commands: Commands,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
) {
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    let admission = lunco_workspace::FileDocumentAdmission::capture(
        workspace.as_deref().map(|workspace| &workspace.0),
        replication.as_ref(),
    );
    let qualified = trigger.qualified.clone();
    commands.queue(move |world: &mut World| {
        drill_into_class(world, &qualified, admission);
    });
}

/// Tab-to-class binding for drill-in tabs whose document hasn't
/// been installed in the registry yet. Stored in
/// [`crate::ui::document_openings::DocumentOpenings`] under
/// [`OpeningState::DrillIn`], valued by the qualified class name
/// the tab is waiting on.
///
/// When the bg task resolves, [`drive_drill_in_loads`] builds a
/// `ModelicaDocument` from the cached AST + source (no second
/// parse) and installs it into the registry, clearing the entry.
pub struct DrillInBinding {
    pub qualified: String,
    /// Off-thread document load. Built via
    /// [`lunco_modelica_core::library_documents::load_library_class`] which
    /// hits the source-library parsed bundle, so a class whose
    /// containing file the engine session has already parsed
    /// installs in milliseconds. Driven by [`drive_drill_in_loads`].
    pub task: bevy::tasks::Task<
        Result<
            (
                lunco_modelica_document::ModelicaDocument,
                lunco_workspace::DocumentRuntimeOwner,
            ),
            String,
        >,
    >,
    /// RAII guard registered with [`lunco_status_core::status_bus::StatusBus`].
    /// Dropped together with the binding (on install or on document
    /// removal) — the bus then clears the
    /// `(BusyScope::Document, "drill-in")` slot via `drain_busy_drops`.
    /// Kept as a field so any future work that wants to query
    /// "is this document loading?" goes through the bus.
    pub busy: lunco_status_core::status_bus::BusyHandle,
}

/// A successful rewrite carries the selected source lifetime through installation.
pub struct PreparedDuplicate {
    document: lunco_modelica_document::ModelicaDocument,
    source_runtime: lunco_workspace::DocumentRuntimeOwner,
    resident: Option<lunco_workspace::PinnedDocumentRuntimeOwner>,
}

impl PreparedDuplicate {
    pub(crate) fn build(
        document: lunco_doc::DocumentId,
        name: String,
        qualified: &str,
        source: crate::ui::class_source::ResolvedClassSource,
    ) -> Result<Self, String> {
        let imports = match source.origin_path.as_deref() {
            Some(path) => crate::ui::duplicate::collect_parent_imports(path)?,
            None => Vec::new(),
        };
        let spans = crate::ui::duplicate::extract_class_spans_inline(&source.source, qualified);
        let rewritten = crate::ui::duplicate::build_duplicate_source(
            &source.source,
            spans.as_ref(),
            &name,
            Some(qualified),
            &imports,
        )?;
        let syntax = lunco_modelica_document::SyntaxCache::from_source(&rewritten, 0);
        if syntax.has_errors() {
            return Err("rewritten duplicate has syntax errors".into());
        }
        let document = lunco_modelica_document::ModelicaDocument::from_parts(
            document,
            rewritten,
            lunco_doc::DocumentOrigin::untitled(name),
            std::sync::Arc::new(syntax),
        );
        Ok(Self {
            document,
            source_runtime: source.runtime,
            resident: source.resident,
        })
    }
}

/// The pending task and exact owners are retired together on cancellation.
pub struct DuplicateBinding {
    pub display_name: String,
    pub origin_short: String,
    pub task: bevy::tasks::Task<Result<PreparedDuplicate, String>>,
    pub target: lunco_workspace::DocumentRuntimeOwner,
    pub source_pin: Option<lunco_workspace::PinnedDocumentRuntimeOwner>,
    pub busy: lunco_status_core::status_bus::BusyHandle,
}

/// Remove only the uninstalled document's metadata and pending view bindings.
fn retire_duplicate_placeholder(
    document: lunco_doc::DocumentId,
    cache: &mut crate::package_tree::PackageTreeCache,
    tabs: &mut crate::model_tabs::ModelTabs,
    workspace: Option<&mut lunco_workspace::WorkspaceResource>,
    commands: &mut Commands,
) {
    cache.in_memory_models.retain(|entry| entry.doc != document);
    for instance in tabs.close_all_for_doc(document) {
        commands.trigger(lunco_workbench_core::commands::CloseTab {
            kind: crate::ui::MODEL_VIEW_KIND,
            instance,
        });
    }
    if let Some(workspace) = workspace {
        workspace.close_document(document);
    }
}

/// Consume the reservation returned by shared cancellation, regardless of which
/// runtime observer removed its pending task first.
pub(crate) fn retire_duplicate_placeholder_in(world: &mut World, document: lunco_doc::DocumentId) {
    if let Some(mut workspace) = world.get_resource_mut::<lunco_workspace::WorkspaceResource>() {
        workspace.close_document(document);
    }
    world.resource_scope(
        |world, mut cache: Mut<crate::package_tree::PackageTreeCache>| {
            world.resource_scope(|world, mut tabs: Mut<crate::model_tabs::ModelTabs>| {
                retire_duplicate_placeholder(
                    document,
                    &mut cache,
                    &mut tabs,
                    None,
                    &mut world.commands(),
                );
            });
        },
    );
}

pub(crate) fn retire_twin_duplicates(
    trigger: On<lunco_workspace::TwinClosed>,
    mut openings: ResMut<DocumentOpenings>,
    mut cache: ResMut<crate::package_tree::PackageTreeCache>,
    mut tabs: ResMut<crate::model_tabs::ModelTabs>,
    mut workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
    mut commands: Commands,
) {
    retire_duplicate_owner(
        &lunco_workspace::DocumentRuntimeOwner::LocalTwin(trigger.twin),
        &mut openings,
        &mut cache,
        &mut tabs,
        &mut workspace,
        &mut commands,
    );
}

pub(crate) fn retire_remote_duplicates(
    trigger: On<lunco_core_session::ReplicationOwnerRetired>,
    mut openings: ResMut<DocumentOpenings>,
    mut cache: ResMut<crate::package_tree::PackageTreeCache>,
    mut tabs: ResMut<crate::model_tabs::ModelTabs>,
    mut workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
    mut commands: Commands,
) {
    retire_duplicate_owner(
        &lunco_workspace::DocumentRuntimeOwner::Replicated(trigger.owner.clone()),
        &mut openings,
        &mut cache,
        &mut tabs,
        &mut workspace,
        &mut commands,
    );
}

fn retire_duplicate_owner(
    owner: &lunco_workspace::DocumentRuntimeOwner,
    openings: &mut DocumentOpenings,
    cache: &mut crate::package_tree::PackageTreeCache,
    tabs: &mut crate::model_tabs::ModelTabs,
    workspace: &mut Option<ResMut<lunco_workspace::WorkspaceResource>>,
    commands: &mut Commands,
) {
    for document in openings.doc_ids() {
        let retired = matches!(openings.get_mut(document), Some(OpeningState::Duplicate(binding))
            if &binding.target == owner || binding.source_pin.as_ref().is_some_and(|pin| &pin.runtime == owner));
        if retired {
            drop(openings.cancel(document));
            retire_duplicate_placeholder(document, cache, tabs, workspace.as_deref_mut(), commands);
        }
    }
}

/// Bevy system: poll pending duplicate bg tasks; `install_prebuilt`
/// the fully-built document into the registry when ready. Same
/// shape as [`drive_drill_in_loads`] but for the `Duplicate to
/// Workspace` flow.
pub fn drive_duplicate_loads(
    mut openings: bevy::prelude::ResMut<DocumentOpenings>,
    mut registry: bevy::prelude::ResMut<ModelicaDocuments>,
    mut cache: ResMut<crate::package_tree::PackageTreeCache>,
    mut probe: Option<bevy::prelude::ResMut<crate::FrameTimeProbe>>,
    mut egui_q: bevy::prelude::Query<&mut bevy_egui::EguiContext>,
    mut tabs: bevy::prelude::ResMut<crate::model_tabs::ModelTabs>,
    mut canvas_state: bevy::prelude::ResMut<super::CanvasDiagramState>,
    mut commands: bevy::prelude::Commands,
    mut workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
) {
    use bevy::prelude::*;
    // While any duplicate is in-flight, ping egui every tick so the
    // canvas keeps repainting and the loading overlay actually
    // animates. Without this the canvas paints once at tab-open then
    // sleeps until something else requests a repaint — the overlay
    // is unreachable for the entire bg-parse window and the user sees
    // a blank canvas (verified via [Overlay] trace: no entries between
    // "ModelView rendering tab" and "duplicate: installed").
    if openings.has_any_duplicate() {
        for mut ctx in egui_q.iter_mut() {
            ctx.get_mut().request_repaint();
        }
    }
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    let doc_ids = openings.doc_ids();
    let mut had_install = false;
    for doc_id in doc_ids {
        if openings.duplicate_display(doc_id).is_none() {
            continue;
        }
        let current_workspace = workspace.as_deref().map(|workspace| &workspace.0);
        let current = if let Some(OpeningState::Duplicate(binding)) = openings.get_mut(doc_id) {
            binding
                .target
                .is_current(current_workspace, replication.as_ref())
                && current_workspace.is_none_or(|workspace| {
                    workspace
                        .document(doc_id)
                        .is_some_and(|document| document.runtime_context == binding.target)
                })
                && binding.source_pin.as_ref().is_none_or(|pin| {
                    pin.is_current(current_workspace, replication.as_ref())
                        && registry.host(pin.document).is_some()
                })
        } else {
            false
        };
        if !current {
            drop(openings.cancel(doc_id));
            let message = format!(
                "Modelica duplicate {doc_id} cancelled: its admitted source or target owner retired"
            );
            warn!("{message}");
            commands.trigger(lunco_core::RuntimeError {
                name: "modelica-duplicate-cancelled".into(),
                message,
            });
            retire_duplicate_placeholder(
                doc_id,
                &mut cache,
                &mut tabs,
                workspace.as_deref_mut(),
                &mut commands,
            );
            continue;
        }
        let t_poll = web_time::Instant::now();
        let polled: Option<Result<PreparedDuplicate, String>> =
            if let Some(OpeningState::Duplicate(b)) = openings.get_mut(doc_id) {
                bevy::tasks::futures_lite::future::block_on(
                    bevy::tasks::futures_lite::future::poll_once(&mut b.task),
                )
            } else {
                None
            };
        let Some(result) = polled else { continue };
        let poll_ms = t_poll.elapsed().as_secs_f64() * 1000.0;
        let Some(OpeningState::Duplicate(b)) = openings.remove(doc_id) else {
            continue;
        };
        let dup_display_name = b.display_name;
        let origin_short = b.origin_short;
        let mut busy = b.busy;
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(message) => {
                warn!("Modelica duplicate {doc_id} failed: {message}");
                commands.trigger(lunco_core::RuntimeError {
                    name: "modelica-duplicate-failed".into(),
                    message: message.clone(),
                });
                busy.set_outcome(lunco_status_core::status_bus::BusyOutcome::Failed(message));
                retire_duplicate_placeholder(
                    doc_id,
                    &mut cache,
                    &mut tabs,
                    workspace.as_deref_mut(),
                    &mut commands,
                );
                continue;
            }
        };
        let current_workspace = workspace.as_deref().map(|workspace| &workspace.0);
        if !prepared
            .source_runtime
            .is_current(current_workspace, replication.as_ref())
            || prepared.resident.as_ref().is_some_and(|pin| {
                !pin.is_current(current_workspace, replication.as_ref())
                    || registry.host(pin.document).is_none()
            })
        {
            let message =
                format!("Modelica duplicate {doc_id} cancelled: its selected source owner retired");
            warn!("{message}");
            commands.trigger(lunco_core::RuntimeError {
                name: "modelica-duplicate-cancelled".into(),
                message,
            });
            busy.set_outcome(lunco_status_core::status_bus::BusyOutcome::Cancelled);
            retire_duplicate_placeholder(
                doc_id,
                &mut cache,
                &mut tabs,
                workspace.as_deref_mut(),
                &mut commands,
            );
            continue;
        }
        let t_install = web_time::Instant::now();
        if let Err(error) = registry.install_prebuilt(doc_id, prepared.document) {
            let message = format!("duplicate document {doc_id} could not be installed: {error}");
            warn!("{message}");
            commands.trigger(lunco_core::RuntimeError {
                name: "modelica-duplicate-failed".into(),
                message: message.clone(),
            });
            busy.set_outcome(lunco_status_core::status_bus::BusyOutcome::Failed(message));
            retire_duplicate_placeholder(
                doc_id,
                &mut cache,
                &mut tabs,
                workspace.as_deref_mut(),
                &mut commands,
            );
            continue;
        }
        // Only a published document may carry its busy handle into projection.
        canvas_state.stash_projection_handoff(doc_id, busy);
        let install_ms = t_install.elapsed().as_secs_f64() * 1000.0;
        info!(
            "[CanvasDiagram] duplicate: installed `{}` (from `{}`) — poll={poll_ms:.1}ms install={install_ms:.1}ms",
            dup_display_name, origin_short,
        );
        had_install = true;
        // Seed the drill-in target so the canvas projects the duplicated
        // model, not the package's empty top-level. The duplicate is
        // always the extracted target class — either a standalone model
        // (`within Pkg; model BarCopy …`) or a whole copied package
        // (`package FooCopy { model Bar … }`). Either way the first
        // non-package class in the copy's Index is the thing to show; its
        // `c.name` is already the within-qualified name, so it resolves
        // directly. Without this the user sees the empty-overlay
        // placeholder card and has to click into the tree manually.
        if let Some(host) = registry.host(doc_id) {
            // Read first non-package class from the per-doc Index;
            // sees optimistic patches and avoids walking the AST.
            let index = host.document().index();
            let qualified = index
                .classes
                .values()
                .find(|c| !matches!(c.kind, lunco_modelica_index::index::ClassKind::Package))
                .map(|c| c.name.clone());
            // Replace the `(doc, None)` placeholder with a fresh tab
            // bound to `(doc, Some(qualified))`. TabId bindings are
            // immutable; mutating drilled_class in place would collapse
            // distinct tabs into duplicate `(doc, drilled)` keys.
            if let Some(q) = qualified {
                let placeholder = tabs
                    .iter_mut_for_doc(doc_id)
                    .find(|(_, s)| s.drilled_class.is_none())
                    .map(|(id, _)| id);
                if let Some(old_id) = placeholder {
                    commands.trigger(lunco_workbench_core::commands::CloseTab {
                        kind: crate::ui::MODEL_VIEW_KIND,
                        instance: old_id,
                    });
                    tabs.close_tab(old_id);
                }
                let new_id = tabs.ensure_for(doc_id, Some(q));
                if let Some(tab) = tabs.get_mut(new_id) {
                    tab.view_mode = crate::model_tabs_types::ModelViewMode::Canvas;
                }
                commands.trigger(lunco_workbench_core::commands::OpenTab {
                    kind: crate::ui::MODEL_VIEW_KIND,
                    instance: new_id,
                });
            }
        }
        // Pre-warm the source library inheritance chain on a dedicated thread so
        // the projection finds inherited connectors. Same pattern as
        // the drill-in path. The duplicated copy carries `within
        // <origin package>;` so the within-prefixed qualified path
        // (e.g. `Modelica.Blocks.Continuous.PIDCopy`) gives the
        // scope-chain resolver enough context to walk up to
        // `Modelica.Blocks.Interfaces.SISO`.
        if let Some(host) = registry.host(doc_id) {
            // Read within-prefix + extends from the Index. Both are
            // pre-extracted during rebuild, so no AST walk per drill-in.
            let index = host.document().index();
            let within_prefix = index.within_path.clone().unwrap_or_default();
            let qpath = if within_prefix.is_empty() {
                dup_display_name.clone()
            } else {
                format!("{within_prefix}.{dup_display_name}")
            };
            // Fall back to the short name when the qualified path
            // isn't directly indexed (e.g. user-typed un-`within`'d
            // top-level classes).
            let entry = index
                .classes
                .get(&qpath)
                .or_else(|| index.classes.get(&dup_display_name));
            // Engine session caches across calls; the projection task
            // resolves inherited components on demand. No off-thread
            // prewarm needed.
            let _ = entry;
        }
    }
    if had_install {
        if let Some(p) = probe.as_deref_mut() {
            p.last_edit = Some(web_time::Instant::now());
        }
    }
}

pub fn drive_drill_in_loads(
    mut openings: bevy::prelude::ResMut<DocumentOpenings>,
    mut registry: bevy::prelude::ResMut<ModelicaDocuments>,
    mut tabs: bevy::prelude::ResMut<crate::model_tabs::ModelTabs>,
    mut egui_q: bevy::prelude::Query<&mut bevy_egui::EguiContext>,
    mut canvas_state: bevy::prelude::ResMut<super::CanvasDiagramState>,
    mut workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    mut commands: Commands,
) {
    use bevy::prelude::*;
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    // Keep egui awake while loads are in flight so the "Loading…"
    // overlay actually animates. Mirrors the duplicate-loads driver
    // — without this the canvas paints once and sleeps until input.
    if openings.has_any_drill_in() {
        for mut ctx in egui_q.iter_mut() {
            ctx.get_mut().request_repaint();
        }
    }
    let doc_ids = openings.doc_ids();
    for doc_id in doc_ids {
        let polled: Option<
            Result<
                (
                    lunco_modelica_document::ModelicaDocument,
                    lunco_workspace::DocumentRuntimeOwner,
                ),
                String,
            >,
        > = if let Some(OpeningState::DrillIn(b)) = openings.get_mut(doc_id) {
            bevy::tasks::futures_lite::future::block_on(
                bevy::tasks::futures_lite::future::poll_once(&mut b.task),
            )
        } else {
            None
        };
        let Some(result) = polled else { continue };
        let Some(OpeningState::DrillIn(b)) = openings.remove(doc_id) else {
            continue;
        };
        let qualified = b.qualified;
        let mut busy = b.busy;
        let (doc, runtime) = match result {
            Ok((doc, runtime)) => {
                if !runtime.is_current(
                    workspace.as_deref().map(|workspace| &workspace.0),
                    replication.as_ref(),
                ) {
                    let message = format!(
                        "Library file load for {qualified} cancelled: source runtime owner retired"
                    );
                    warn!("{message}");
                    commands.trigger(lunco_core::RuntimeError {
                        name: "modelica-file-open-cancelled".into(),
                        message,
                    });
                    busy.set_outcome(lunco_status_core::status_bus::BusyOutcome::Cancelled);
                    continue;
                }
                // Success path: hand the parse-phase busy handle to
                // the canvas state so the bus keeps a `Document(d)`
                // entry continuously through the parse→project
                // transition. Released by `complete_projection_handoff`
                // once the projection spawn mints its own.
                canvas_state.stash_projection_handoff(doc_id, busy);
                (doc, runtime)
            }
            Err(msg) => {
                warn!(
                    "[CanvasDiagram] drill-in: class `{}` load failed: {}",
                    qualified, msg
                );
                // Drop the handle with `Failed` outcome — no handoff,
                // no projection. The bus records the outcome under
                // `(Document(d), "drill-in")`; the canvas overlay
                // picks it up via `bus.lifecycle(...) →
                // LifecycleState::Failed(msg)` and renders the
                // drill-in error overlay. No per-tab `load_error`
                // plumbing needed.
                busy.set_outcome(lunco_status_core::status_bus::BusyOutcome::Failed(msg));
                drop(busy);
                continue;
            }
        };
        // Capture file path for the install log + smart-view decision
        // before moving the doc into the registry.
        let (file_path_display, has_components) = {
            let path = match doc.origin() {
                lunco_doc::DocumentOrigin::File { path, .. } => path.display().to_string(),
                _ => String::from("<no path>"),
            };
            // Smart default view for the drilled-in tab. Matches
            // OMEdit/Dymola: icon-only class or class with zero
            // instantiated components → Icon view; otherwise Canvas
            // (the user drilled FROM a canvas, expects a canvas).
            let has_components = doc.strict_ast().and_then(|ast| {
                lunco_modelica_index::class_lookup::find_class_by_qualified_name(&ast, &qualified)
                    .map(|c| !c.components.is_empty())
            });
            (path, has_components)
        };
        let origin = doc.origin().clone();
        if let Err(error) = registry.install_prebuilt(doc_id, doc) {
            bevy::log::warn!(
                "[CanvasDiagram] drill-in document {doc_id} could not be installed: {error}"
            );
            continue;
        }
        if let Some(workspace) = workspace.as_deref_mut() {
            lunco_modelica_core::doc_ops::register_document_context(
                workspace, doc_id, origin, runtime, false,
            );
        }
        let land_in_icon_view = crate::ui::class_display::is_icon_only_class(&qualified)
            || has_components == Some(false);
        if land_in_icon_view {
            // Update the drilled-in tab's view mode. Multiple tabs
            // may now point at the same doc (sibling drill-ins);
            // scope by `(doc, qualified)`.
            if let Some(tab) = tabs.find_for_mut(doc_id, Some(qualified.as_str())) {
                tab.view_mode = crate::model_tabs_types::ModelViewMode::Icon;
            }
        }
        info!(
            "[CanvasDiagram] drill-in: installed `{}` from `{}`",
            qualified, file_path_display,
        );
    }
}

/// Open the Modelica class with `qualified` name in a new tab.
/// The tab appears immediately with an empty document showing a
/// "Loading…" overlay; the file read happens on a background task
/// and the source is applied via `ReplaceSource` when the read
/// completes. This matches what users expect: the tab opens, a
/// spinner says "loading", content lands when it's ready.
pub fn drill_into_class(
    world: &mut World,
    qualified: &str,
    admission: lunco_workspace::FileDocumentAdmission,
) {
    // Opening a Modelica class is a workbench navigation action, not merely a
    // tab allocation. A generated network can be opened from the simulation
    // Build perspective; switch to the Modelica perspective before creating
    // the tab so the user lands on the diagram instead of the 3D viewport.
    // The concrete workbench shell is extracted while egui panels render, so
    // navigation must use the workbench's deferred command boundary rather than touching
    // the resource directly. The workbench drains perspective requests before
    // tab requests, preserving this ordering even when the gesture originated
    // inside a panel render.
    world
        .commands()
        .trigger(lunco_workbench_core::commands::ActivatePerspective {
            id: "modelica_analyze".into(),
        });

    // On web the source library *source* tar is unpacked lazily: the fast-path bundle
    // install registers only the parsed AST (`GLOBAL_PARSED_SOURCE_BUNDLE`), leaving
    // the source files stashed compressed. The path resolvers below query
    // `global_library_sources()`, which is empty for source files until that
    // unpack runs — so without this, clicking any source library class in the library
    // tree (or a canvas drill-in) silently no-ops on web. Unpack now;
    // it's idempotent and one-time. Native already has the sources on disk.
    #[cfg(target_arch = "wasm32")]
    lunco_modelica_library::source_library::ensure_library_source_unpacked();

    // Try source library paths first (resolves Modelica.* and any other source library-rooted
    // qualified path). Fallback: scan the open document registry for a
    // doc whose AST contains the requested class — handles non-source library
    // user-opened files (e.g. `assets/models/AnnotatedRocketStage.mo`)
    // where the qualified name lives only in a workspace document.
    let file_path = crate::library_fs::resolve_class_path_indexed(qualified)
        .or_else(|| crate::library_fs::locate_library_file(qualified));
    if let Some(file_path) = file_path {
        open_drill_in_tab(world, qualified, &file_path, admission);
        return;
    }
    // Open-document fallback: find a host whose parsed AST resolves the
    // qualified path. Reuse its tab + just set the drill-in class. Shares
    // the "which open doc owns this class" rule with the by-name source
    // resolver (duplicate) via the one helper.
    let target_doc = crate::ui::class_source::find_open_doc_with_class(world, qualified);
    if let Some(doc_id) = target_doc {
        // Allocate (or focus) a tab dedicated to this `(doc, class)`.
        // Distinct sibling classes from the same `.mo` file get their
        // own tabs — that's the whole point of keying ModelTabs by
        // TabId rather than DocumentId.
        let tab_id = {
            let mut tabs = world.resource_mut::<crate::model_tabs::ModelTabs>();
            // Drill-in is a deliberate navigation gesture (canvas
            // double-click), so the tab is pinned via ensure_for —
            // not the preview slot. Same-class re-drill focuses;
            // sibling drills still get their own tabs.
            let tab_id = tabs.ensure_for(doc_id, Some(qualified.to_string()));
            if let Some(tab) = tabs.get_mut(tab_id) {
                tab.view_mode = crate::model_tabs_types::ModelViewMode::Canvas;
            }
            tab_id
        };
        // `ensure_for(doc_id, Some(qualified))` immediately above
        // already wrote it.
        if let Some(mut workspace) = world.get_resource_mut::<lunco_workspace::WorkspaceResource>()
        {
            workspace.active_document = Some(doc_id);
        }
        world
            .commands()
            .trigger(lunco_workbench_core::commands::OpenTab {
                kind: crate::ui::MODEL_VIEW_KIND,
                instance: tab_id,
            });
        bevy::log::info!(
            "[CanvasDiagram] drill-in: opened tab #{tab_id} for `{}` on existing doc",
            qualified,
        );
        return;
    }
    // External example: a LunCoSim model in the asset library
    // (`assets/models/*.mo`). This
    // is the third `SourceRootKind` (`Bundled`), so routing it here means
    // `OpenClass{qualified}` resolves the WHOLE schema — source library, open workspace
    // docs, AND bundled demos — through one command instead of the Welcome
    // panel owning a separate bundled opener. Match the top-level qualified
    // segment against a bundled model's filename stem and open it in-memory.
    let stem = lunco_modelica_ast::qualified_name_segments(qualified)
        .next()
        .unwrap_or(qualified);
    if crate::models::bundled_models().is_ok_and(|models| {
        models
            .iter()
            .any(|m| m.filename.trim_end_matches(".mo") == stem)
    }) {
        bevy::log::info!("[CanvasDiagram] drill-in: opening bundled `{stem}`");
        let admission = crate::ui::panels::package_browser::capture_file_admission(world);
        crate::ui::panels::package_browser::open_class(
            world,
            crate::class_ref::ClassRef::bundled([stem]),
            true,
            admission,
        );
        return;
    }
    bevy::log::warn!(
        "[CanvasDiagram] drill-in: could not locate `{}` (no source library match, no open doc, no bundled model)",
        qualified
    );
}

/// Open a tab for `qualified` class backed by a **placeholder
/// document** — empty source, parses instantly. Spawns a bg task
/// that reads the file; a later Bevy system applies `ReplaceSource`
/// when the read completes.
///
/// The user sees:
///  1. Instant: a new tab titled with the class short name.
///  2. Immediately: an "Loading…" overlay on the canvas.
///  3. A moment later: the real source + diagram populates.
///
/// If a tab for the same file path is already open (from a
/// previous drill-in), we focus it instead of making a second.
fn open_drill_in_tab(
    world: &mut World,
    qualified: &str,
    file_path: &std::path::Path,
    admission: lunco_workspace::FileDocumentAdmission,
) {
    // Find or allocate the doc. Reuse an existing one only if the
    // same `(file, drilled-in class)` was opened before — keying on
    // file alone collapsed sibling source library classes (e.g. `Integrator`
    // and `Derivative` both in `Continuous.mo`) onto one tab, so a
    // second drill silently focused the first tab instead of
    // showing the requested class.
    let model_path_id = format!("library://{qualified}");
    let existing_doc = {
        let registry = world.resource::<ModelicaDocuments>();
        let tabs = world.resource::<crate::model_tabs::ModelTabs>();
        // A tab whose `(doc.file, drilled_class)` matches the new
        // request — re-focus it instead of allocating a duplicate.
        tabs.iter().find_map(|(_id, state)| {
            if state.drilled_class.as_deref() != Some(qualified) {
                return None;
            }
            let same_file = registry
                .host(state.doc)
                .and_then(|h| match h.document().origin() {
                    lunco_doc::DocumentOrigin::File { path, .. } => Some(path == file_path),
                    _ => None,
                })
                .unwrap_or(false);
            same_file.then_some(state.doc)
        })
    };
    if let Some(document) = existing_doc {
        let replication = lunco_core_session::current_replication_owner_in(world);
        let workspace = world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .map(|workspace| &workspace.0);
        let live = lunco_workspace::PinnedDocumentRuntimeOwner::for_document(document, workspace)
            .is_ok_and(|pin| pin.is_current(workspace, replication.as_ref()));
        if !live {
            let message = format!(
                "Library document {document} belongs to a retired runtime owner; reopen its source explicitly"
            );
            warn!("{message}");
            world.commands().trigger(lunco_core::RuntimeError {
                name: "modelica-file-open-failed".into(),
                message,
            });
            return;
        }
    }
    let (doc_id, needs_load) = if let Some(id) = existing_doc {
        (id, false)
    } else {
        // Reserve a doc id only; the actual `ModelicaDocument`
        // (including the rumoca parse) is built on a background
        // thread and installed via `install_prebuilt` when ready.
        // Queries against the id before install return `None`;
        // panels render the "Loading resource…" overlay via
        // `StatusBus::is_busy(BusyScope::Document(doc.0))` — the
        // `DrillInBinding` minted at spawn keeps the bus entry alive.
        let registry = world.resource_mut::<ModelicaDocuments>();
        let id = registry.reserve_id();
        (id, true)
    };

    if needs_load {
        // Spawn the off-thread load. `load_library_class` extracts only
        // the target class from the wrapper file: a 152 KB
        // `Modelica/Blocks/package.mo` becomes a ~7 KB doc holding
        // just `PID_Controller` + a `within Modelica.Blocks.Examples;`
        // prefix for scope-chain resolution. Lazy doc — no main-
        // thread parse on install; `drive_engine_sync` parses the
        // small slice off-thread. Driver: `drive_drill_in_loads`.
        let path_for_task = file_path.to_path_buf();
        let qualified_for_task = qualified.to_string();
        let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
            lunco_modelica_core::library_documents::load_library_class(
                doc_id,
                &path_for_task,
                &qualified_for_task,
                admission,
            )
            .await
        });
        let busy = {
            let mut bus = world.resource_mut::<lunco_status_core::status_bus::StatusBus>();
            bus.begin(
                lunco_status_core::status_bus::BusyScope::Document(doc_id.0),
                "drill-in",
                format!("Loading {qualified}"),
            )
        };
        let mut openings = world.resource_mut::<DocumentOpenings>();
        openings.insert(
            doc_id,
            OpeningState::DrillIn(DrillInBinding {
                qualified: qualified.to_string(),
                task,
                busy,
            }),
        );
    }
    // call below is in the same stack frame, so no observer can
    // run between this point and the tab carrying the drilled
    // scope — the original race the eager bind protected against
    // doesn't exist when the source-of-truth IS the tab.

    let _ = model_path_id;

    // Register the tab + land the user in Canvas view (they
    // drilled FROM a canvas, so the canvas is what they expect
    // to see). Default `view_mode` is Text for newly-created
    // scratch models; drill-in is a different use case.
    let tab_id = {
        let mut model_tabs = world.resource_mut::<crate::model_tabs::ModelTabs>();
        let tab_id = model_tabs.ensure_for(doc_id, Some(qualified.to_string()));
        if let Some(tab) = model_tabs.get_mut(tab_id) {
            tab.view_mode = crate::model_tabs_types::ModelViewMode::Canvas;
        }
        tab_id
    };
    world
        .commands()
        .trigger(lunco_workbench_core::commands::OpenTab {
            kind: crate::ui::MODEL_VIEW_KIND,
            instance: tab_id,
        });

    bevy::log::info!(
        "[CanvasDiagram] drill-in: opened placeholder tab for `{}` (file: `{}`) — loading in background",
        qualified,
        file_path.display()
    );
}

#[cfg(test)]
mod duplicate_retirement_tests {
    use super::*;

    #[test]
    fn duplicate_owner_observers_cancel_only_exact_pending_lifetimes() {
        use lunco_workspace::{
            DocumentRuntimeOwner, PinnedDocumentRuntimeOwner, ReplicatedSceneOwner,
            ReplicationOwner, TwinId,
        };
        let pool = bevy::tasks::TaskPoolBuilder::new().num_threads(1).build();
        let mut app = App::new();
        app.init_resource::<DocumentOpenings>()
            .init_resource::<crate::model_tabs::ModelTabs>()
            .init_resource::<lunco_workspace::WorkspaceResource>()
            .init_resource::<lunco_status_core::status_bus::StatusBus>()
            .add_observer(retire_twin_duplicates)
            .add_observer(retire_remote_duplicates)
            .add_systems(Update, lunco_status_core::status_bus::drain_busy_drops);
        app.insert_resource(crate::package_tree::PackageTreeCache {
            roots: Vec::new(),
            tasks: Vec::new(),
            in_memory_models: Vec::new(),
            bundled_tree_indexed: false,
            library_roots_synced: false,
        });
        let connection = app.world_mut().spawn_empty().id();
        let replacement = app.world_mut().spawn_empty().id();
        let remote = ReplicationOwner::Twin {
            scene: ReplicatedSceneOwner {
                connection,
                host_twin: TwinId::new(20),
                authority: "generic-mount".into(),
                root: std::path::PathBuf::new(),
                owns_mount: true,
            },
        };
        let mut other_connection = remote.clone();
        if let ReplicationOwner::Twin { scene } = &mut other_connection {
            scene.connection = replacement;
        }
        let mut other_mount = remote.clone();
        if let ReplicationOwner::Twin { scene } = &mut other_mount {
            scene.host_twin = TwinId::new(21);
        }
        let twin_a = DocumentRuntimeOwner::LocalTwin(TwinId::new(1));
        let admissions = [
            (twin_a.clone(), None),
            (
                DocumentRuntimeOwner::Application,
                Some(PinnedDocumentRuntimeOwner {
                    document: lunco_doc::DocumentId::new(900),
                    runtime: twin_a,
                }),
            ),
            (DocumentRuntimeOwner::LocalTwin(TwinId::new(2)), None),
            (DocumentRuntimeOwner::Replicated(remote.clone()), None),
            (DocumentRuntimeOwner::Replicated(other_connection), None),
            (DocumentRuntimeOwner::Replicated(other_mount), None),
            (DocumentRuntimeOwner::Application, None),
        ];
        for (index, (target, source_pin)) in admissions.into_iter().enumerate() {
            let document = lunco_doc::DocumentId::new(index as u64 + 1);
            let name = format!("Pending{}", document.0);
            app.world_mut()
                .resource_mut::<crate::package_tree::PackageTreeCache>()
                .in_memory_models
                .push(lunco_modelica_index::package_tree::types::InMemoryEntry {
                    display_name: name.clone(),
                    id: format!("mem://{name}"),
                    doc: document,
                });
            app.world_mut()
                .resource_mut::<crate::model_tabs::ModelTabs>()
                .ensure_for(document, None);
            lunco_modelica_core::doc_ops::register_document_context(
                &mut app
                    .world_mut()
                    .resource_mut::<lunco_workspace::WorkspaceResource>()
                    .0,
                document,
                lunco_doc::DocumentOrigin::untitled(name.clone()),
                target.clone(),
                false,
            );
            let busy = app
                .world_mut()
                .resource_mut::<lunco_status_core::status_bus::StatusBus>()
                .begin(
                    lunco_status_core::status_bus::BusyScope::Document(document.0),
                    "duplicate",
                    "Preparing",
                );
            app.world_mut().resource_mut::<DocumentOpenings>().insert(
                document,
                OpeningState::Duplicate(DuplicateBinding {
                    display_name: name,
                    origin_short: "Source".into(),
                    target,
                    source_pin,
                    task: pool.spawn(std::future::pending::<Result<PreparedDuplicate, String>>()),
                    busy,
                }),
            );
        }
        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin: TwinId::new(1),
            root: std::path::PathBuf::new(),
            was_active: true,
        });
        app.world_mut()
            .trigger(lunco_core_session::ReplicationOwnerRetired { owner: remote });
        app.update();
        for raw in 1..=7 {
            let document = lunco_doc::DocumentId::new(raw);
            let retained = ![1, 2, 4].contains(&raw);
            assert_eq!(
                app.world()
                    .resource::<DocumentOpenings>()
                    .in_flight
                    .contains_key(&document),
                retained
            );
            assert_eq!(
                app.world()
                    .resource::<crate::package_tree::PackageTreeCache>()
                    .in_memory_models
                    .iter()
                    .any(|entry| entry.doc == document),
                retained
            );
            assert_eq!(
                app.world()
                    .resource::<crate::model_tabs::ModelTabs>()
                    .any_for_doc(document)
                    .is_some(),
                retained
            );
            assert_eq!(
                app.world()
                    .resource::<lunco_workspace::WorkspaceResource>()
                    .document(document)
                    .is_some(),
                retained
            );
            assert_eq!(
                app.world()
                    .resource::<lunco_status_core::status_bus::StatusBus>()
                    .is_busy(lunco_status_core::status_bus::BusyScope::Document(raw)),
                retained
            );
        }
    }

    #[test]
    fn duplicate_cancellation_returns_the_owned_reservation_for_cleanup() {
        let pool = bevy::tasks::TaskPoolBuilder::new().num_threads(1).build();
        let mut world = World::new();
        world.init_resource::<DocumentOpenings>();
        world.init_resource::<crate::model_tabs::ModelTabs>();
        world.init_resource::<lunco_workspace::WorkspaceResource>();
        world.insert_resource(crate::package_tree::PackageTreeCache {
            roots: Vec::new(),
            tasks: Vec::new(),
            in_memory_models: Vec::new(),
            bundled_tree_indexed: false,
            library_roots_synced: false,
        });
        let document = lunco_doc::DocumentId::new(7);
        let unrelated = lunco_doc::DocumentId::new(8);
        let mut bus = lunco_status_core::status_bus::StatusBus::default();
        for id in [document, unrelated] {
            let name = format!("Copy{}", id.0);
            world
                .resource_mut::<crate::package_tree::PackageTreeCache>()
                .in_memory_models
                .push(lunco_modelica_index::package_tree::types::InMemoryEntry {
                    display_name: name.clone(),
                    id: format!("mem://{name}"),
                    doc: id,
                });
            world
                .resource_mut::<crate::model_tabs::ModelTabs>()
                .ensure_for(id, None);
            lunco_modelica_core::doc_ops::register_document_context(
                &mut world.resource_mut::<lunco_workspace::WorkspaceResource>().0,
                id,
                lunco_doc::DocumentOrigin::untitled(name.clone()),
                lunco_workspace::DocumentRuntimeOwner::Application,
                false,
            );
            world.resource_mut::<DocumentOpenings>().insert(
                id,
                OpeningState::Duplicate(DuplicateBinding {
                    display_name: name,
                    origin_short: "Source".into(),
                    task: pool.spawn(std::future::pending::<Result<PreparedDuplicate, String>>()),
                    target: lunco_workspace::DocumentRuntimeOwner::Application,
                    source_pin: None,
                    busy: bus.begin(
                        lunco_status_core::status_bus::BusyScope::Document(id.0),
                        "duplicate",
                        "Preparing",
                    ),
                }),
            );
        }
        // A different runtime owner may remove the task before its duplicate
        // observer runs. The returned state still identifies the cleanup consumer.
        let cancelled = world.resource_mut::<DocumentOpenings>().cancel(document);
        assert!(matches!(cancelled, Some(OpeningState::Duplicate(_))));
        drop(cancelled);
        retire_duplicate_placeholder_in(&mut world, document);
        assert!(
            world
                .resource::<DocumentOpenings>()
                .in_flight
                .contains_key(&unrelated)
        );
        assert!(
            !world
                .resource::<DocumentOpenings>()
                .in_flight
                .contains_key(&document)
        );
        let cache = world.resource::<crate::package_tree::PackageTreeCache>();
        assert_eq!(cache.in_memory_models.len(), 1);
        assert_eq!(cache.in_memory_models[0].doc, unrelated);
        assert!(
            world
                .resource::<crate::model_tabs::ModelTabs>()
                .any_for_doc(document)
                .is_none()
        );
        assert!(
            world
                .resource::<crate::model_tabs::ModelTabs>()
                .any_for_doc(unrelated)
                .is_some()
        );
        let workspace = world.resource::<lunco_workspace::WorkspaceResource>();
        assert!(workspace.document(document).is_none());
        assert!(workspace.document(unrelated).is_some());
        assert!(
            world
                .resource_mut::<DocumentOpenings>()
                .cancel(document)
                .is_none()
        );
    }
}
