//! Modelica side of workbench document hot-exit (VSCode-style session
//! restore).
//!
//! Registers a [`DocumentSessionCodec`] so `lunco-workbench` can capture
//! every persistable Modelica document's **live editor buffer** into the
//! per-Twin `workspace-state` file and recreate it on next launch — the
//! buffer is the source of truth, so unsaved edits survive a restart.
//!
//! Restore replays [`ModelicaDocumentRegistry::allocate_with_origin`],
//! which pushes `pending_opened` → the existing open pipeline registers
//! the Workspace entry and opens the tab. So this codec stays tiny: read
//! buffers out, push buffers back in, let the normal machinery do the
//! rest.

use std::collections::HashMap;

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_workbench::{
    finalize_revision, revision_term, DocumentSessionCodec, DocumentSnapshot, OpenTab,
};

use crate::model_tabs::ModelTabs;
use crate::state::{is_generated_document, is_generated_origin, ModelicaDocumentRegistry};
use crate::ui::panels::canvas_diagram::CanvasDiagramState;
use crate::ui::MODEL_VIEW_KIND;

const KIND: &str = "modelica";

fn is_persistable_snapshot(snapshot: &DocumentSnapshot) -> bool {
    !(snapshot.kind == KIND && is_generated_origin(&snapshot.origin))
}

fn restore_origin(origin: &lunco_doc::DocumentOrigin) -> lunco_doc::DocumentOrigin {
    match origin {
        lunco_doc::DocumentOrigin::File { path, .. }
            if lunco_assets::msl::owns_filesystem_path(path) =>
        {
            lunco_doc::DocumentOrigin::File {
                path: path.clone(),
                writable: false,
            }
        }
        _ => origin.clone(),
    }
}

/// Per-domain hot-exit codec for Modelica documents.
pub struct ModelicaSessionCodec;

impl DocumentSessionCodec for ModelicaSessionCodec {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn revision(&self, world: &World) -> u64 {
        let Some(reg) = world.get_resource::<ModelicaDocumentRegistry>() else {
            return 0;
        };
        // Order-independent fold of (id, generation) so the gate fires on
        // any edit (generation bump) or open/close, without cloning text.
        let mut acc = 0u64;
        let mut count = 0u64;
        for (id, host) in reg.iter() {
            // Generated USD projections are runtime artifacts, not open editor
            // documents. Including them here makes every scene projection
            // churn the hot-exit revision and causes the next launch to restore
            // stale solver wrappers that no longer belong to the active Twin.
            if is_generated_document(host.document()) {
                continue;
            }
            acc ^= revision_term(id.raw(), host.document().generation_owned());
            count += 1;
        }
        // Fold the per-doc canvas camera (quantized) so a pan/zoom — which
        // doesn't bump the document generation — still re-fires the persist
        // gate and re-saves `view_state` (5c). Quantizing keeps the easing
        // animation from writing on every intermediate frame; the
        // content-compare in the persist system catches the rest.
        if let Some(cds) = world.get_resource::<CanvasDiagramState>() {
            for doc in cds.iter_doc_ids() {
                if let Some(s) = cds.get_for_doc(doc) {
                    let vp = &s.canvas.viewport;
                    let q = |f: f32| (f * 64.0) as i64 as u64;
                    acc ^= q(vp.zoom).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        ^ q(vp.center.x).rotate_left(17)
                        ^ q(vp.center.y).rotate_left(31)
                        ^ doc.raw().wrapping_mul(0x100_0000_01b3);
                }
            }
        }
        finalize_revision(acc, count)
    }

    fn capture(&self, world: &mut World) -> Vec<(u64, DocumentSnapshot)> {
        // Per-doc canvas camera, snapshotted up front so the immutable
        // `CanvasDiagramState` borrow is released before we touch the
        // document registry. Serialized into `view_state` so a reopened
        // diagram restores its exact zoom/pan (5c). `Viewport` is serde.
        let views: HashMap<DocumentId, serde_json::Value> = world
            .get_resource::<CanvasDiagramState>()
            .map(|cds| {
                cds.iter_doc_ids()
                    .filter_map(|d| {
                        let registry = world.get_resource::<ModelicaDocumentRegistry>()?;
                        let host = registry.host(d)?;
                        if is_generated_document(host.document()) {
                            return None;
                        }
                        cds.get_for_doc(d)
                            .and_then(|s| serde_json::to_value(&s.canvas.viewport).ok())
                            .map(|v| (d, v))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Each doc's dock tab instance (ModelTabs id), so 5a can remap the
        // persisted dock tree's `TabId::Instance` ids — this is a SEPARATE
        // counter from `DocumentId.raw()`.
        let tab_ids: HashMap<DocumentId, u64> = world
            .get_resource::<ModelTabs>()
            .map(|tabs| {
                world
                    .get_resource::<ModelicaDocumentRegistry>()
                    .map(|reg| {
                        reg.iter()
                            .filter_map(|(id, host)| {
                                (!is_generated_document(host.document()))
                                    .then(|| tabs.primary_tab_for(id).map(|t| (id, t)))
                                    .flatten()
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        let Some(reg) = world.get_resource::<ModelicaDocumentRegistry>() else {
            return Vec::new();
        };
        reg.iter()
            .filter(|(_, host)| !is_generated_document(host.document()))
            .map(|(id, host)| {
                let doc = host.document();
                let origin = doc.origin().clone();
                let view_state = views.get(&id).cloned().unwrap_or(serde_json::Value::Null);
                (
                    id.raw(),
                    DocumentSnapshot {
                        kind: KIND.to_string(),
                        title: origin.display_name(),
                        source: doc.source_snapshot(),
                        dirty: doc.is_dirty(),
                        origin,
                        id: id.raw(),
                        tab_instance: tab_ids.get(&id).copied().unwrap_or(0),
                        view_state,
                    },
                )
            })
            .collect()
    }

    fn restore(&self, world: &mut World, snap: &DocumentSnapshot) -> Option<u64> {
        // Generated USD documents are transient projection artifacts. Older
        // workspace-state files may still contain them from before the
        // persistence boundary was enforced; drop those snapshots during
        // restore so one clean save removes the stale state permanently.
        if !is_persistable_snapshot(snap) {
            return None;
        }
        // `allocate_with_origin` registers the document and fires
        // `DocumentOpened` — which adds the Workspace entry — but it does
        // NOT open a model-view tab. In normal use the package browser
        // opens the tab via `OpenTab` after a click (see
        // `open_bundled_class`); on session restore there is no click, so
        // we open it here ourselves. Without this the restored doc lives
        // in the registry with no visible tab and the centre shows only
        // Welcome. The saved camera is applied in `apply_view_state`.
        let origin = restore_origin(&snap.origin);
        let new_id = world
            .get_resource_mut::<ModelicaDocumentRegistry>()?
            .allocate_with_origin(snap.source.clone(), origin);
        let tab_id = world.resource_mut::<ModelTabs>().ensure_for(new_id, None);
        world.commands().trigger(OpenTab {
            kind: MODEL_VIEW_KIND,
            instance: tab_id,
        });
        Some(new_id.raw())
    }

    fn apply_view_state(&self, world: &mut World, live_id: u64, snap: &DocumentSnapshot) {
        // Restore the diagram's zoom/pan (5c). `Viewport` is serde; null
        // view_state (no saved camera) deserializes to Err → skip.
        let Ok(view) = serde_json::from_value::<lunco_canvas::Viewport>(snap.view_state.clone())
        else {
            return;
        };
        let doc = DocumentId::new(live_id);
        let Some(mut cds) = world.get_resource_mut::<CanvasDiagramState>() else {
            return;
        };
        if cds.has_entry(doc) {
            // Tab already projected (an auto-opened / already-rendered
            // doc) — snap the live camera now; the initial-projection
            // path won't re-run for it.
            if let Some(ds) = cds.get_mut_for_doc(doc) {
                ds.canvas.viewport.snap_to(view.center, view.zoom);
            }
        } else {
            // Freshly restored: the tab doesn't exist yet (async open).
            // Stash so `get_mut_for_tab` seeds it and the initial
            // projection snaps to it instead of fitting.
            cds.stash_pending_view(doc, view);
        }
    }

    fn instance_remap(
        &self,
        world: &mut World,
        snap: &DocumentSnapshot,
        live_id: u64,
    ) -> Option<(u64, u64)> {
        // Map the saved dock tab instance (old ModelTabs id) to the live one
        // `restore` just opened for this doc. `restore` calls
        // `ensure_for(doc, None)`, so the live primary tab exists; look it up
        // read-only. No valid id recorded (0, e.g. an older saved workspace) → nothing to
        // remap.
        if snap.tab_instance == 0 {
            return None;
        }
        let doc = DocumentId::new(live_id);
        let new_inst = world.get_resource::<ModelTabs>()?.primary_tab_for(doc)?;
        Some((snap.tab_instance, new_inst))
    }

    fn dock_tab_kind(&self) -> Option<&'static str> {
        Some(MODEL_VIEW_KIND.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn snapshot(origin: lunco_doc::DocumentOrigin) -> DocumentSnapshot {
        DocumentSnapshot {
            kind: KIND.to_string(),
            title: "test".to_string(),
            source: "model Test end Test;".to_string(),
            dirty: false,
            origin,
            id: 1,
            tab_instance: 0,
            view_state: serde_json::Value::Null,
        }
    }

    #[test]
    fn generated_documents_are_not_persisted_or_restored() {
        let generated = snapshot(lunco_doc::DocumentOrigin::Bundled {
            filename: "generated/Traverse_System.mo".to_string(),
        });
        let authored = snapshot(lunco_doc::DocumentOrigin::File {
            path: PathBuf::from("/tmp/Test.mo"),
            writable: true,
        });

        assert!(!is_persistable_snapshot(&generated));
        assert!(is_persistable_snapshot(&authored));

        let mut world = World::new();
        let mut registry = ModelicaDocumentRegistry::default();
        registry.allocate_with_origin(
            generated.source.clone(),
            generated.origin.clone(),
        );
        registry.allocate_with_origin(authored.source.clone(), authored.origin.clone());
        world.insert_resource(registry);

        let captured = ModelicaSessionCodec.capture(&mut world);
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].1.origin, authored.origin);
    }
}
