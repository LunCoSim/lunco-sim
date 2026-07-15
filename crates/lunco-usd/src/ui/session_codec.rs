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
use lunco_workbench::{finalize_revision, revision_term, DocumentSessionCodec, DocumentSnapshot};

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
                acc ^= revision_term(id.raw(), host.generation());
                count += 1;
            }
        }
        finalize_revision(acc, count)
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
                            // USD docs aren't model-view dock tabs, so no
                            // tab instance to remap (5a).
                            tab_instance: 0,
                            // USD has no canvas-zoom equivalent to persist.
                            view_state: serde_json::Value::Null,
                        },
                    )
                })
            })
            .collect()
    }

    fn restore(&self, world: &mut World, snap: &DocumentSnapshot) -> Option<u64> {
        // Restoring a document's SOURCE from the workspace-state cache is correct ONLY
        // for a networked CLIENT: it received the scene from the host over the wire and
        // has no local file, so the cached buffer is its only copy. In Standalone (local
        // — the DEFAULT) and Host mode the on-disk file is authoritative and reopened
        // from disk (a doc-backed twin scene is rebuilt from its base file + runtime
        // overlay by `drain_pending_twin_docs`). Reading the cache off-client would
        // SHADOW an externally-edited file with a stale buffer — the bug where the
        // moonbase scene rendered a pre-migration version and ignored disk edits. So the
        // local/host build never looks at the cache; it re-reads the file every open.
        let is_client =
            matches!(world.get_resource::<lunco_core::NetworkRole>(), Some(lunco_core::NetworkRole::Client));
        if !is_client {
            return None;
        }
        let mut reg = world.get_resource_mut::<UsdDocumentRegistry>()?;
        // Fires `DocumentOpened` → the USD UI's stage registration.
        let id = reg.allocate(snap.source.clone(), snap.origin.clone());
        Some(id.raw())
    }
}
