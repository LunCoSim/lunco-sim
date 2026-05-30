//! USD side of workbench document hot-exit (VSCode-style session
//! restore) — the mirror of `lunco-modelica`'s `session_codec`.
//!
//! Registers a [`DocumentSessionCodec`] so `lunco-workbench` captures
//! every open USD document's **live buffer** into the per-Twin
//! `workspace-state` file and recreates it on next launch. Restore
//! replays [`UsdDocumentRegistry::allocate`], which fires the
//! `DocumentOpened` lifecycle the USD UI already reacts to (the
//! [`WorkspaceStage`](super::WorkspaceStage) registration).
//!
//! Uses only the registry's existing public surface (`ids` + `host`,
//! `allocate`, `DocumentHost::generation`, `UsdDocument::{source,
//! origin, is_dirty}`) — no changes to the registry or document types.

use bevy::prelude::*;
use lunco_workbench::{DocumentSessionCodec, DocumentSnapshot};

use crate::registry::UsdDocumentRegistry;

const KIND: &str = "usd";

/// Per-domain hot-exit codec for USD documents.
pub struct UsdSessionCodec;

impl DocumentSessionCodec for UsdSessionCodec {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn revision(&self, world: &World) -> u64 {
        let Some(reg) = world.get_resource::<UsdDocumentRegistry>() else {
            return 0;
        };
        // Order-independent fold of (id, generation): changes on any
        // edit (generation bump via `DocumentHost::generation`) or
        // open/close, without cloning text.
        let mut acc = 0u64;
        let mut count = 0u64;
        for id in reg.ids().collect::<Vec<_>>() {
            if let Some(host) = reg.host(id) {
                let gen = host.generation();
                acc ^= id
                    .raw()
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .rotate_left((gen & 63) as u32)
                    ^ gen.wrapping_mul(0x1000_0000_01b3);
                count += 1;
            }
        }
        acc.wrapping_add(count.wrapping_mul(0x100_0000_01b3))
    }

    fn capture(&self, world: &mut World) -> Vec<(u64, DocumentSnapshot)> {
        let Some(reg) = world.get_resource::<UsdDocumentRegistry>() else {
            return Vec::new();
        };
        reg.ids()
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|id| {
                reg.host(id).map(|host| {
                    let doc = host.document();
                    let origin = doc.origin().clone();
                    (
                        id.raw(),
                        DocumentSnapshot {
                            kind: KIND.to_string(),
                            title: origin.display_name(),
                            source: doc.source().to_string(),
                            dirty: doc.is_dirty(),
                            origin,
                            id: id.raw(),
                            // USD has no canvas-zoom equivalent to persist.
                            view_state: serde_json::Value::Null,
                        },
                    )
                })
            })
            .collect()
    }

    fn restore(&self, world: &mut World, snap: &DocumentSnapshot) -> Option<u64> {
        let mut reg = world.get_resource_mut::<UsdDocumentRegistry>()?;
        // Fires `DocumentOpened` → the USD UI's stage registration.
        let id = reg.allocate(snap.source.clone(), snap.origin.clone());
        Some(id.raw())
    }
}
