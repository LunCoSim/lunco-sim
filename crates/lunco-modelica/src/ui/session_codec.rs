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

use bevy::prelude::*;
use lunco_workbench::{DocumentSessionCodec, DocumentSnapshot};

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
        acc.wrapping_add(count.wrapping_mul(0x100_0000_01b3))
    }

    fn capture(&self, world: &mut World) -> Vec<(u64, DocumentSnapshot)> {
        let Some(reg) = world.get_resource::<ModelicaDocumentRegistry>() else {
            return Vec::new();
        };
        reg.iter()
            .map(|(id, host)| {
                let doc = host.document();
                let origin = doc.origin().clone();
                (
                    id.raw(),
                    DocumentSnapshot {
                        kind: KIND.to_string(),
                        title: origin.display_name(),
                        source: doc.source_snapshot(),
                        dirty: doc.is_dirty(),
                        origin,
                    },
                )
            })
            .collect()
    }

    fn restore(&self, world: &mut World, snap: &DocumentSnapshot) {
        if let Some(mut reg) = world.get_resource_mut::<ModelicaDocumentRegistry>() {
            // Replays the open pipeline (tab + workspace entry) for free.
            reg.allocate_with_origin(snap.source.clone(), snap.origin.clone());
        }
    }
}
