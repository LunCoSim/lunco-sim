//! Modelica side of workbench document hot-exit (VSCode-style session
//! restore).
//!
//! Registers a [`DocumentSessionCodec`] so `lunco-workbench` can capture
//! every open Modelica document's **live editor buffer** into the
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
use lunco_workbench::{DocumentSessionCodec, DocumentSnapshot, OpenTab};

use crate::ui::panels::canvas_diagram::CanvasDiagramState;
use crate::ui::panels::model_view::{ModelTabs, MODEL_VIEW_KIND};
use crate::ui::state::ModelicaDocumentRegistry;

const KIND: &str = "modelica";

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
            let gen = host.document().generation_owned();
            acc ^= id
                .raw()
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .rotate_left((gen & 63) as u32)
                ^ gen.wrapping_mul(0x1000_0000_01b3);
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
                    acc ^= q(vp.zoom)
                        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        ^ q(vp.center.x).rotate_left(17)
                        ^ q(vp.center.y).rotate_left(31)
                        ^ doc.raw().wrapping_mul(0x100_0000_01b3);
                }
            }
        }
        acc.wrapping_add(count.wrapping_mul(0x100_0000_01b3))
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
                        cds.get_for_doc(d)
                            .and_then(|s| serde_json::to_value(&s.canvas.viewport).ok())
                            .map(|v| (d, v))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let Some(reg) = world.get_resource::<ModelicaDocumentRegistry>() else {
            return Vec::new();
        };
        reg.iter()
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
                        view_state,
                    },
                )
            })
            .collect()
    }

    fn restore(&self, world: &mut World, snap: &DocumentSnapshot) -> Option<u64> {
        // `allocate_with_origin` registers the document and fires
        // `DocumentOpened` — which adds the Workspace entry — but it does
        // NOT open a model-view tab. In normal use the package browser
        // opens the tab via `OpenTab` after a click (see
        // `open_bundled_class`); on session restore there is no click, so
        // we open it here ourselves. Without this the restored doc lives
        // in the registry with no visible tab and the centre shows only
        // Welcome. The saved camera is applied in `apply_view_state`.
        let new_id = world
            .get_resource_mut::<ModelicaDocumentRegistry>()?
            .allocate_with_origin(snap.source.clone(), snap.origin.clone());
        let tab_id = world.resource_mut::<ModelTabs>().ensure_for(new_id, None);
        world
            .commands()
            .trigger(OpenTab { kind: MODEL_VIEW_KIND, instance: tab_id });
        Some(new_id.raw())
    }

    fn apply_view_state(&self, world: &mut World, live_id: u64, snap: &DocumentSnapshot) {
        // Restore the diagram's zoom/pan (5c). `Viewport` is serde; null
        // view_state (no saved camera) deserializes to Err → skip.
        let Ok(view) =
            serde_json::from_value::<lunco_canvas::Viewport>(snap.view_state.clone())
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
            let ds = cds.get_mut(Some(doc));
            ds.canvas.viewport.snap_to(view.center, view.zoom);
        } else {
            // Freshly restored: the tab doesn't exist yet (async open).
            // Stash so `get_mut_for_tab` seeds it and the initial
            // projection snaps to it instead of fitting.
            cds.stash_pending_view(doc, view);
        }
    }
}
