//! Helpers for syncing tab state with global workspace state.

use bevy::prelude::*;
use std::collections::HashMap;
use lunco_doc::DocumentId;
use crate::ui::panels::code_editor::EditorBufferState;
use crate::ui::{ModelicaDocumentRegistry, WorkbenchState};
use super::types::{TabRenderContext};
use super::tabs::ModelTabs;

pub fn drilled_class_for_doc(
    world: &World,
    doc: DocumentId,
) -> Option<String> {
    if let Some(ctx) = world.get_resource::<TabRenderContext>() {
        if let Some(tab_id) = ctx.tab_id {
            let tabs = world.resource::<ModelTabs>();
            if let Some(state) = tabs.get(tab_id) {
                if state.doc == doc {
                    return state.drilled_class.clone();
                }
            }
        }
    }
    world.resource::<ModelTabs>().drilled_class_for_doc(doc)
}

/// The class a doc's simulation surfaces default to, in precedence order:
///   1. drilled-in class — the UI drill-in pin; the user is looking at a
///      leaf model and expects *that* to run, not the enclosing package.
///   2. tier-ranked simulation root — `simulation_candidates()[0]`, where an
///      `experiment(...)`-annotated, non-partial class sorts first. This is
///      NOT arbitrary `HashMap` order: a package whose only annotated model
///      is `RoverThermalSystem` must not default to e.g. `LunarEnvironment`.
/// Returns `None` when the doc has no host or no simulatable candidate.
///
/// Single source of truth for "which class does a simulation surface default
/// to" — the Fast Run popup, the Experiments Setup form, and (for the
/// non-ambiguous path) `dispatch_experiment` all route through here so the
/// precedence can't drift between surfaces. Callers that need to disambiguate
/// multiple candidates (e.g. open a picker modal) layer that on top.
pub fn default_simulation_class(world: &World, doc: DocumentId) -> Option<String> {
    let candidates = world
        .get_resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.host(doc))
        .map(|h| h.document().index().simulation_candidates())
        .unwrap_or_default();
    // Run-target override (the Experiments / Fast Run class dropdown) wins,
    // but ONLY when it still names a real candidate — a stale override left
    // over from before a source edit must not silently run a missing class.
    // It is deliberately separate from the drill pin so choosing a run
    // target never moves the canvas view. Falls back to the drill pin, then
    // the tier-ranked default (`sim_target::default_class`).
    let override_cls = world
        .get_resource::<RunTargetOverrides>()
        .and_then(|o| o.0.get(&doc).cloned())
        .filter(|c| candidates.iter().any(|x| x == c));
    if let Some(c) = override_cls {
        return Some(c);
    }
    let drilled = drilled_class_for_doc(world, doc);
    crate::sim_target::default_class(drilled.as_deref(), &candidates)
}

/// Per-document explicit run target, set by the class dropdowns on the
/// Experiments Setup form and the Fast Run modal. Read by
/// [`default_simulation_class`] with top precedence. Kept separate from the
/// drill pin ([`ModelTabs`] `drilled_class`) so picking what to *run* does
/// not change what the canvas *shows*.
#[derive(Resource, Default)]
pub struct RunTargetOverrides(pub HashMap<DocumentId, String>);

/// Pin `class` as the explicit run target for `doc`. Every run surface
/// (Experiments Setup, Fast Run modal, `dispatch_experiment`) re-resolves to
/// it via [`default_simulation_class`]; the canvas drill view is untouched.
pub fn set_run_target_for_doc(world: &mut World, doc: DocumentId, class: &str) {
    if let Some(mut targets) = world.get_resource_mut::<RunTargetOverrides>() {
        targets.0.insert(doc, class.to_string());
    }
}

pub fn resolve_tab_target(world: &World, instance: u64) -> (DocumentId, Option<String>) {
    if let Some(state) = world.get_resource::<ModelTabs>().and_then(|t| t.get(instance)) {
        return (state.doc, state.drilled_class.clone());
    }
    (DocumentId::new(instance), None)
}

pub fn resolve_tab_title(
    world: &World,
    doc: DocumentId,
    drilled_class: Option<&str>,
) -> (String, bool, bool) {
    if let Some(host) = world
        .get_resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.host(doc))
    {
        let document = host.document();
        let base = drilled_class
            .and_then(|qualified| qualified.rsplit('.').next().map(str::to_string))
            .unwrap_or_else(|| {
                // No explicit drill — title reflects the *container*
                // (file name for on-disk docs, origin slug for
                // Untitled drafts), not the inner class name.
                // Renaming the doc row updates the origin and is
                // visible immediately without touching source.
                // The inner class can be renamed separately via the
                // M-badge row → `RenameModelicaClass`.
                let raw = document.origin().display_name();
                if raw == "package" {
                    if let lunco_doc::DocumentOrigin::File { path, .. } =
                        document.origin()
                    {
                        if let Some(parent) = path
                            .parent()
                            .and_then(|p| p.file_name())
                            .and_then(|s| s.to_str())
                        {
                            return parent.to_string();
                        }
                    }
                }
                raw
            });
        return (base, document.is_dirty(), document.is_read_only());
    }

    let active_doc = world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document);
    if active_doc == Some(doc) {
        if let Some(name) = crate::ui::state::display_name_for(world, doc) {
            return (name, false, crate::ui::state::read_only_for(world, doc));
        }
    }
    (format!("Model #{}", doc.raw()), false, false)
}

pub fn sync_active_tab_to_doc(
    world: &mut World,
    doc: DocumentId,
    _drilled_class: Option<&str>,
) {
    let active_matches = world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document)
        == Some(doc);
    // Fast-path: if we're already active AND the buffer is already bound
    // to this doc with the same generation, nothing to do.
    let buffer_matches = {
        let registry = world.resource::<ModelicaDocumentRegistry>();
        let live_gen = registry.host(doc).map(|h| h.generation()).unwrap_or(0);
        let buf = world.get_resource::<EditorBufferState>();
        let buf_doc = buf.and_then(|b| b.bound_doc);
        let buf_gen = buf.map(|b| b.generation).unwrap_or(0);
        buf_doc == Some(doc) && buf_gen == live_gen
    };
    if active_matches && buffer_matches {
        refresh_selected_entity_for(world, doc);
        return;
    }

    let snapshot = {
        let registry = world.resource::<ModelicaDocumentRegistry>();
        registry.host(doc).map(|h| {
            let document = h.document();
            let display_name = document.origin().display_name();
            let path_str = document
                .canonical_path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("mem://{display_name}"));
            let library = match document.origin() {
                lunco_doc::DocumentOrigin::Untitled { .. } => {
                    crate::ui::state::ModelLibrary::InMemory
                }
                lunco_doc::DocumentOrigin::Bundled { .. } => {
                    crate::ui::state::ModelLibrary::Bundled
                }
                lunco_doc::DocumentOrigin::File { writable: true, .. } => {
                    crate::ui::state::ModelLibrary::User
                }
                lunco_doc::DocumentOrigin::File { writable: false, .. } => {
                    crate::ui::state::ModelLibrary::Bundled
                }
            };
            let read_only =
                matches!(library, crate::ui::state::ModelLibrary::Bundled);
            let detected_name = document
                .index()
                .classes
                .values()
                .find(|c| !matches!(c.kind, crate::index::ClassKind::Package))
                .map(|c| c.name.clone());
            (
                path_str,
                display_name,
                document.source().to_string(),
                read_only,
                library,
                detected_name,
            )
        })
    };

    let snapshot = snapshot.or_else(|| {
        let openings = world
            .get_resource::<crate::ui::document_openings::DocumentOpenings>()?;
        if let Some(qualified) = openings.drill_in_qualified(doc) {
            let qualified = qualified.to_string();
            let short = qualified
                .rsplit('.')
                .next()
                .map(str::to_string)
                .unwrap_or_else(|| qualified.clone());
            return Some((
                format!("msl://{qualified}"),
                short.clone(),
                String::new(),
                true,
                crate::ui::state::ModelLibrary::Bundled,
                Some(short),
            ));
        }
        if let Some(display) = openings.duplicate_display(doc) {
            let display = display.to_string();
            return Some((
                format!("mem://{display}"),
                display.clone(),
                String::new(),
                false,
                crate::ui::state::ModelLibrary::InMemory,
                Some(display),
            ));
        }
        None
    });
    let Some((path_str, display_name, source, read_only, library, detected_name)) =
        snapshot
    else {
        return;
    };

    let _ = (display_name, read_only, library);
    {
        let source_arc: std::sync::Arc<str> = source.clone().into();
        let mut state = world.resource_mut::<WorkbenchState>();
        state.editor_buffer = source_arc.to_string();
    }

    {
        let mut ws = world.resource_mut::<lunco_workbench::WorkspaceResource>();
        ws.active_document = Some(doc);
    }

    // Editor buffer sync removed.
    //
    // This function used to overwrite `EditorBufferState.{text,
    // detected_name, model_path, bound_doc}` from `doc.source()`
    // every frame. That was the legacy push-from-doc-to-buffer
    // pipeline; it ran *before* `CodeEditorPanel::render` and
    // clobbered any uncommitted typing whenever the mismatch
    // condition tripped. The new pipeline is:
    //
    // - `editor_on_doc_changed` observer — push-driven, fires on
    //   `DocumentChanged`, syncs the bound doc's buffer from
    //   `doc.source()`. Replaces the per-frame mismatch poll.
    // - `code_editor::render` tab-switch branch — handles initial
    //   load + per-pane snapshot/restore when the user clicks a
    //   different tab.
    //
    // Both paths track `generation` correctly; this site doesn't
    // need to participate.
    let _ = (path_str, detected_name);

    refresh_selected_entity_for(world, doc);
}

pub fn refresh_selected_entity_for(world: &mut World, doc: DocumentId) {
    let entity = world
        .resource::<ModelicaDocumentRegistry>()
        .entities_linked_to(doc)
        .into_iter()
        .next();
    if let Some(entity) = entity {
        if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
            if state.selected_entity != Some(entity) {
                state.selected_entity = Some(entity);
            }
        }
    }
}
