//! USD side of workbench document hot-exit (VSCode-style session restore).
//!
//! Open USD documents and their primary preview tabs are stored in the
//! per-Twin `workspace-state` file. Dirty buffers restore from that snapshot;
//! clean file-backed buffers refresh from disk during background preparation.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use bevy::prelude::*;
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_doc_bevy::DocumentRegistry;
use lunco_usd_document::document::{LayerId, UsdDocument};
use lunco_usd_viewport_core::{
    CloseUsdPreview, OpenUsdPreview, OpenUsdPreviewView, USD_PREVIEW_VIEW_PANEL_KIND, UsdPreviewId,
    UsdPreviewViewSettings, UsdViewportState,
};
use lunco_workbench_state::{
    DocumentSessionCodec, DocumentSnapshot, PreparedDocumentSnapshot, finalize_revision,
    revision_term,
};

const KIND: &str = "usd";
const USD_VIEW_STATE_VERSION: u64 = 1;

/// Per-domain hot-exit codec for USD documents.
pub struct UsdSessionCodec;

#[derive(Resource, Default)]
struct UsdSessionRestoreRemaps(HashMap<u64, Vec<(u64, u64)>>);

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedUsdPreviewView {
    id: u64,
    settings: UsdPreviewViewSettings,
}

fn restore_usd_document(world: &mut World, snapshot: &DocumentSnapshot) -> Option<u64> {
    let mut registry = world.get_resource_mut::<DocumentRegistry<UsdDocument>>()?;
    let (id, outcome): (DocumentId, Option<lunco_doc::OpenOutcome>) = match &snapshot.origin {
        DocumentOrigin::File { path, writable } => {
            let (id, outcome) =
                registry.open_file_with_writable(path.clone(), snapshot.source.clone(), *writable);
            (id, Some(outcome))
        }
        DocumentOrigin::Bundled { filename } => {
            let id = registry.find_bundled(filename).unwrap_or_else(|| {
                registry.restore(snapshot.source.clone(), snapshot.origin.clone())
            });
            (id, None)
        }
        DocumentOrigin::Untitled { .. } => (
            registry.restore(snapshot.source.clone(), snapshot.origin.clone()),
            None,
        ),
    };
    if matches!(outcome, Some(lunco_doc::OpenOutcome::KeptUnparsable)) {
        warn!(
            "[WorkspaceState] could not restore saved USD source for {}; keeping the open document",
            snapshot.title
        );
        return None;
    }
    if snapshot.dirty {
        if let Some(host) = registry.host_mut(id) {
            host.document_mut().mark_restored_dirty();
        }
    }
    Some(id.raw())
}

impl DocumentSessionCodec for UsdSessionCodec {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn revision(&self, world: &World) -> u64 {
        let Some(reg) = world.get_resource::<DocumentRegistry<UsdDocument>>() else {
            return 0;
        };
        let mut acc = 0u64;
        let mut count = 0u64;
        for id in reg.ids() {
            if let Some(host) = reg.host(id) {
                let generation = host
                    .generation()
                    .wrapping_mul(2)
                    .wrapping_add(u64::from(host.document().is_dirty()));
                acc ^= revision_term(id.raw(), generation);
                count += 1;
            }
        }
        if let Some(viewport) = world.get_resource::<UsdViewportState>() {
            for session in viewport.sessions() {
                let edit_target_hash = session
                    .edit_target()
                    .as_str()
                    .bytes()
                    .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
                        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
                    });
                acc ^= revision_term(session.id().0, edit_target_hash);
                count += 1;
            }
            for view in viewport.views() {
                let settings = UsdPreviewViewSettings::capture(view);
                let quantize = |value: f32| (value * 64.0) as i64 as u64;
                let mut view_revision = quantize(settings.yaw)
                    ^ quantize(settings.pitch).rotate_left(7)
                    ^ quantize(settings.distance).rotate_left(13)
                    ^ quantize(settings.target[0]).rotate_left(19)
                    ^ quantize(settings.target[1]).rotate_left(29)
                    ^ quantize(settings.target[2]).rotate_left(37)
                    ^ quantize(settings.orthographic_scale).rotate_left(43)
                    ^ (settings.projection as u64).rotate_left(3)
                    ^ (settings.mode as u64).rotate_left(11)
                    ^ (settings.text_layer as u64).rotate_left(23)
                    ^ u64::from(settings.auto_frame).rotate_left(31);
                view_revision ^= view.preview().0.rotate_left(47);
                acc ^= revision_term(view.id().0, view_revision);
                count += 1;
            }
        }
        finalize_revision(acc, count)
    }

    fn capture(&self, world: &mut World) -> Vec<(u64, DocumentSnapshot)> {
        let Some(reg) = world.get_resource::<DocumentRegistry<UsdDocument>>() else {
            return Vec::new();
        };
        let viewport = world.get_resource::<UsdViewportState>();
        reg.ids()
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|id| {
                reg.host(id).map(|host| {
                    let doc = host.document();
                    let origin = doc.origin().clone();
                    let session = viewport.and_then(|viewport| {
                        viewport
                            .preview_for_document(id)
                            .and_then(|preview| viewport.session(preview))
                    });
                    let mut saved_views = session.map_or_else(Vec::new, |session| {
                        let mut views = viewport
                            .into_iter()
                            .flat_map(|viewport| viewport.views())
                            .filter(|view| view.preview() == session.id())
                            .map(|view| SavedUsdPreviewView {
                                id: view.id().0,
                                settings: UsdPreviewViewSettings::capture(view),
                            })
                            .collect::<Vec<_>>();
                        if !views.iter().any(|view| view.id == session.primary_view().0) {
                            views.push(SavedUsdPreviewView {
                                id: session.primary_view().0,
                                settings: UsdPreviewViewSettings::default(),
                            });
                        }
                        views
                    });
                    if let Some(session) = session {
                        saved_views.sort_unstable_by_key(|view| {
                            (view.id != session.primary_view().0, view.id)
                        });
                    }
                    (
                        id.raw(),
                        DocumentSnapshot {
                            kind: KIND.to_string(),
                            title: origin.display_name(),
                            source: doc.source(),
                            dirty: doc.is_dirty(),
                            origin,
                            id: id.raw(),
                            tab_instance: session
                                .map(|session| session.primary_view().0)
                                .unwrap_or(0),
                            view_state: session.map_or_else(
                                || {
                                    if viewport.is_some_and(|viewport| {
                                        viewport.preview_was_explicitly_closed(id)
                                    }) {
                                        serde_json::json!({
                                            "version": USD_VIEW_STATE_VERSION,
                                            "views": []
                                        })
                                    } else {
                                        serde_json::Value::Null
                                    }
                                },
                                |session| {
                                    serde_json::json!({
                                        "version": USD_VIEW_STATE_VERSION,
                                        "edit_target": session.edit_target().as_str(),
                                        "views": saved_views,
                                    })
                                },
                            ),
                        },
                    )
                })
            })
            .collect()
    }

    fn prepare_restore(
        &self,
        world: &World,
        snapshot: DocumentSnapshot,
    ) -> Pin<Box<dyn Future<Output = PreparedDocumentSnapshot> + Send>> {
        let is_client = matches!(
            world.get_resource::<lunco_core_session::NetworkRole>(),
            Some(lunco_core_session::NetworkRole::Client)
        );
        let file_path = match &snapshot.origin {
            DocumentOrigin::File { path, .. } => Some(path.clone()),
            DocumentOrigin::Untitled { .. } | DocumentOrigin::Bundled { .. } => None,
        };
        Box::pin(async move {
            if is_client || snapshot.dirty {
                return PreparedDocumentSnapshot::new(snapshot);
            }
            let Some(path) = file_path else {
                return PreparedDocumentSnapshot::new(snapshot);
            };
            match lunco_storage::read_text_file_sync(&path) {
                Ok(source) => {
                    let mut snapshot = snapshot;
                    snapshot.source = source;
                    PreparedDocumentSnapshot::new(snapshot)
                }
                Err(error) => {
                    let mut snapshot = snapshot;
                    snapshot.dirty = true;
                    PreparedDocumentSnapshot::with_warning(
                        snapshot,
                        format!(
                            "could not refresh clean USD session source {} from disk ({error}); preserving its saved buffer as dirty",
                            path.display()
                        ),
                    )
                }
            }
        })
    }

    fn restore(&self, world: &mut World, snapshot: &DocumentSnapshot) -> Option<u64> {
        restore_usd_document(world, snapshot)
    }

    fn restore_existing(
        &self,
        world: &mut World,
        snapshot: &DocumentSnapshot,
        live_id: u64,
    ) -> Option<u64> {
        if !matches!(snapshot.origin, DocumentOrigin::File { .. }) {
            return Some(live_id);
        }
        let restored = restore_usd_document(world, snapshot)?;
        (restored == live_id).then_some(restored)
    }

    fn apply_view_state(&self, world: &mut World, live_id: u64, snapshot: &DocumentSnapshot) {
        if world.get_resource::<UsdViewportState>().is_none() {
            return;
        }
        let doc = DocumentId::new(live_id);
        let edit_target = snapshot
            .view_state
            .get("edit_target")
            .and_then(serde_json::Value::as_str)
            .map(LayerId::new)
            .unwrap_or_else(LayerId::root);
        let mut restore_default_view = snapshot.view_state.is_null();
        let view_values = if restore_default_view {
            Vec::new()
        } else {
            if snapshot
                .view_state
                .get("version")
                .and_then(serde_json::Value::as_u64)
                != Some(USD_VIEW_STATE_VERSION)
            {
                warn!("[WorkspaceState] unsupported USD view-state version");
                restore_default_view = true;
                Vec::new()
            } else if let Some(views) = snapshot
                .view_state
                .get("views")
                .and_then(serde_json::Value::as_array)
            {
                views.clone()
            } else {
                warn!("[WorkspaceState] USD view state is missing its views list");
                restore_default_view = true;
                Vec::new()
            }
        };
        let had_saved_view_records = !view_values.is_empty();
        let mut saved_views = Vec::new();
        for value in view_values {
            match serde_json::from_value::<SavedUsdPreviewView>(value) {
                Ok(view) if view.id != 0 => saved_views.push(view),
                Ok(_) => warn!("[WorkspaceState] skipping USD view with id 0"),
                Err(error) => warn!("[WorkspaceState] skipping invalid USD view state: {error}"),
            }
        }
        if had_saved_view_records && saved_views.is_empty() {
            restore_default_view = true;
        }
        if snapshot.tab_instance != 0
            && !saved_views
                .iter()
                .any(|view| view.id == snapshot.tab_instance)
        {
            saved_views.push(SavedUsdPreviewView {
                id: snapshot.tab_instance,
                settings: UsdPreviewViewSettings::default(),
            });
        }
        saved_views.sort_unstable_by_key(|view| (view.id != snapshot.tab_instance, view.id));
        saved_views.dedup_by_key(|view| view.id);
        if saved_views.is_empty() && !restore_default_view {
            let preview = {
                let mut viewport = world.resource_mut::<UsdViewportState>();
                viewport.suppress_auto_preview_for(doc);
                viewport.mark_preview_closed(doc);
                viewport.preview_for_document(doc)
            };
            if let Some(preview) = preview {
                world.trigger(CloseUsdPreview { preview });
            }
            return;
        }
        let (preview, remaps) = {
            let mut viewport = world.resource_mut::<UsdViewportState>();
            let preview = viewport
                .preview_for_document(doc)
                .unwrap_or_else(|| UsdPreviewId::for_document(doc));
            let primary_view = if let Some(session) = viewport.session(preview) {
                session.primary_view()
            } else if let Some(view) = viewport.reserve_view_id() {
                view
            } else {
                warn!("[WorkspaceState] USD preview view identity space is exhausted");
                return;
            };
            let mut remaps = Vec::new();
            if let Some(saved_primary) = saved_views.first() {
                viewport.queue_restore_settings(primary_view, saved_primary.settings.clone());
                remaps.push((saved_primary.id, primary_view));
            } else {
                viewport.queue_restore_settings(primary_view, UsdPreviewViewSettings::default());
            }
            for old_view in saved_views.into_iter().skip(1) {
                let Some(view) = viewport.reserve_view_id() else {
                    warn!("[WorkspaceState] USD preview view identity space is exhausted");
                    return;
                };
                viewport.queue_restore_settings(view, old_view.settings);
                remaps.push((old_view.id, view));
            }
            viewport.queue_restore_primary_view(preview, primary_view);
            (preview, remaps)
        };
        if !remaps.is_empty() {
            world.init_resource::<UsdSessionRestoreRemaps>();
            world.resource_mut::<UsdSessionRestoreRemaps>().0.insert(
                live_id,
                remaps.iter().map(|(old, new)| (*old, new.0)).collect(),
            );
        }
        world.trigger(OpenUsdPreview {
            preview,
            doc_id: doc,
            edit_target,
        });
        for (_, view) in remaps.into_iter().skip(1) {
            world.trigger(OpenUsdPreviewView { preview, view });
        }
    }

    fn owns_tab_instance(&self, snapshot: &DocumentSnapshot, instance: u64) -> bool {
        snapshot.tab_instance == instance
            || snapshot
                .view_state
                .get("views")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|views| {
                    views.iter().any(|view| {
                        view.get("id").and_then(serde_json::Value::as_u64) == Some(instance)
                    })
                })
    }

    fn instance_remaps(
        &self,
        world: &mut World,
        _snapshot: &DocumentSnapshot,
        live_id: u64,
    ) -> Vec<(u64, u64)> {
        let Some(mut remaps) = world.get_resource_mut::<UsdSessionRestoreRemaps>() else {
            return Vec::new();
        };
        remaps.0.remove(&live_id).unwrap_or_default()
    }

    fn dock_tab_kind(&self) -> Option<&'static str> {
        Some(USD_PREVIEW_VIEW_PANEL_KIND)
    }

    fn discard_unmapped_dock_tab_kind(&self) -> Option<&'static str> {
        Some(USD_PREVIEW_VIEW_PANEL_KIND)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_usd_viewport_core::{
        UsdPreviewId, UsdPreviewSession, UsdPreviewView, UsdPreviewViewId, UsdViewportState,
    };

    fn snapshot(dirty: bool) -> DocumentSnapshot {
        DocumentSnapshot {
            kind: KIND.into(),
            origin: DocumentOrigin::File {
                path: std::path::PathBuf::from("/tmp/session-test/scene.usda"),
                writable: true,
            },
            title: "scene.usda".into(),
            source: "#usda 1.0\n".into(),
            dirty,
            id: 4,
            tab_instance: 17,
            view_state: serde_json::Value::Null,
        }
    }

    #[test]
    fn restores_local_usd_buffer_and_dirty_state() {
        let mut app = App::new();
        app.init_resource::<DocumentRegistry<UsdDocument>>();

        let snapshot = snapshot(true);
        let id = UsdSessionCodec
            .restore(app.world_mut(), &snapshot)
            .expect("a local USD snapshot is restored");
        let host = app
            .world()
            .resource::<DocumentRegistry<UsdDocument>>()
            .host(DocumentId::new(id))
            .expect("restored USD document is registered");

        assert_eq!(host.document().source(), snapshot.source);
        assert!(host.document().is_dirty());
    }

    #[test]
    fn restores_one_document_for_path_spellings_of_the_same_file() {
        let mut app = App::new();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        let temp = tempfile::tempdir().expect("temporary USD file");
        let path = temp.path().join("scene.usda");
        lunco_storage::write_file_sync(&path, b"#usda 1.0\n").expect("USD source exists");

        let first = snapshot(true);
        let id = UsdSessionCodec
            .restore(
                app.world_mut(),
                &DocumentSnapshot {
                    origin: DocumentOrigin::writable_file(path.clone()),
                    ..first.clone()
                },
            )
            .expect("the saved USD document is restored");
        let second_path = temp.path().join(".").join("scene.usda");
        let mut second = first;
        second.origin = DocumentOrigin::writable_file(second_path);
        second.source = "#usda 1.0\ndef Xform \"Newer copy\" {}\n".into();

        let same_id = UsdSessionCodec
            .restore(app.world_mut(), &second)
            .expect("same-file restore reuses the document");
        let registry = app.world().resource::<DocumentRegistry<UsdDocument>>();
        let host = registry
            .host(DocumentId::new(id))
            .expect("canonical document remains open");

        assert_eq!(same_id, id);
        assert_eq!(registry.ids().count(), 1);
        assert_eq!(host.document().source(), "#usda 1.0\n");
        assert!(host.document().is_dirty());
    }

    #[test]
    fn restores_dirty_saved_buffer_into_an_already_open_clean_file() {
        let mut app = App::new();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        let temp = tempfile::tempdir().expect("temporary USD file");
        let path = temp.path().join("scene.usda");
        lunco_storage::write_file_sync(&path, b"#usda 1.0\n").expect("USD source exists");
        let (live, _) = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .open_file(path.clone(), "#usda 1.0\n".into());
        let mut snapshot = snapshot(true);
        snapshot.origin = DocumentOrigin::writable_file(path);
        snapshot.source = "#usda 1.0\ndef Xform \"Saved edit\" {}\n".into();

        assert_eq!(
            UsdSessionCodec.restore_existing(app.world_mut(), &snapshot, live.raw()),
            Some(live.raw())
        );
        let registry = app.world().resource::<DocumentRegistry<UsdDocument>>();
        let document = registry
            .host(live)
            .expect("the original identity remains")
            .document();

        assert_eq!(registry.ids().count(), 1);
        assert!(document.is_dirty());
        assert!(document.source().contains("Saved edit"));
    }

    #[test]
    fn restores_preview_tab_with_a_new_live_view_identity() {
        let mut app = App::new();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        app.init_resource::<UsdViewportState>();
        let mut snapshot = snapshot(true);
        let mut second_settings = UsdPreviewViewSettings::default();
        second_settings.yaw = 1.25;
        snapshot.view_state = serde_json::json!({
            "version": USD_VIEW_STATE_VERSION,
            "edit_target": "@runtime@",
            "views": [
                {"id": 17, "settings": UsdPreviewViewSettings::default()},
                {"id": 18, "settings": second_settings},
            ],
        });
        let id = UsdSessionCodec
            .restore(app.world_mut(), &snapshot)
            .expect("a local USD snapshot is restored");

        UsdSessionCodec.apply_view_state(app.world_mut(), id, &snapshot);
        assert_eq!(
            UsdSessionCodec.instance_remaps(app.world_mut(), &snapshot, id),
            vec![(17, 1), (18, 2)]
        );
        assert_eq!(
            app.world_mut()
                .resource_mut::<UsdViewportState>()
                .take_restore_primary_view(UsdPreviewId::for_document(DocumentId::new(id))),
            Some(UsdPreviewViewId(1))
        );
        assert!(UsdSessionCodec.owns_tab_instance(&snapshot, 18));
        let mut pending = app
            .world()
            .resource::<UsdViewportState>()
            .pending_restore_views();
        pending.sort_unstable_by_key(|view| view.0);
        assert_eq!(pending, vec![UsdPreviewViewId(1), UsdPreviewViewId(2)]);
        assert_eq!(
            app.world()
                .resource::<DocumentRegistry<UsdDocument>>()
                .ids()
                .count(),
            1
        );
        assert_eq!(
            UsdSessionCodec.dock_tab_kind(),
            Some(USD_PREVIEW_VIEW_PANEL_KIND)
        );
    }

    #[test]
    fn restores_legacy_usd_session_with_a_single_default_preview() {
        let mut app = App::new();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        app.init_resource::<UsdViewportState>();
        let mut snapshot = snapshot(true);
        snapshot.tab_instance = 0;
        snapshot.view_state = serde_json::Value::Null;
        let id = UsdSessionCodec
            .restore(app.world_mut(), &snapshot)
            .expect("legacy USD document restores");

        UsdSessionCodec.apply_view_state(app.world_mut(), id, &snapshot);

        assert!(
            UsdSessionCodec
                .instance_remaps(app.world_mut(), &snapshot, id)
                .is_empty()
        );
        assert_eq!(
            app.world()
                .resource::<UsdViewportState>()
                .pending_restore_views(),
            vec![UsdPreviewViewId(1)]
        );
        assert_eq!(
            app.world()
                .resource::<DocumentRegistry<UsdDocument>>()
                .ids()
                .count(),
            1
        );
    }

    #[test]
    fn captures_an_explicit_empty_view_list_after_the_last_view_was_closed() {
        let mut app = App::new();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        app.init_resource::<UsdViewportState>();
        let doc = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .restore(
                "#usda 1.0\n".into(),
                DocumentOrigin::File {
                    path: std::path::PathBuf::from("/tmp/session-test/scene.usda"),
                    writable: true,
                },
            );
        app.world_mut()
            .resource_mut::<UsdViewportState>()
            .mark_preview_closed(doc);

        let snapshot = UsdSessionCodec
            .capture(app.world_mut())
            .into_iter()
            .find(|(id, _)| *id == doc.raw())
            .map(|(_, snapshot)| snapshot)
            .expect("USD document is captured");

        assert_eq!(snapshot.view_state["version"], 1);
        assert_eq!(snapshot.view_state["views"], serde_json::json!([]));
    }

    #[test]
    fn an_opening_document_without_a_preview_is_not_captured_as_explicitly_closed() {
        let mut app = App::new();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        app.init_resource::<UsdViewportState>();
        let doc = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .restore(
                "#usda 1.0\n".into(),
                DocumentOrigin::File {
                    path: std::path::PathBuf::from("/tmp/session-test/loading.usda"),
                    writable: true,
                },
            );

        let snapshot = UsdSessionCodec
            .capture(app.world_mut())
            .into_iter()
            .find(|(id, _)| *id == doc.raw())
            .map(|(_, snapshot)| snapshot)
            .expect("USD document is captured");

        assert!(snapshot.view_state.is_null());
    }

    #[test]
    fn restored_closed_preview_suppresses_automatic_reopen() {
        let mut app = App::new();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        app.init_resource::<UsdViewportState>();
        let mut snapshot = snapshot(true);
        snapshot.tab_instance = 0;
        snapshot.view_state = serde_json::json!({
            "version": USD_VIEW_STATE_VERSION,
            "views": [],
        });
        let id = UsdSessionCodec
            .restore(app.world_mut(), &snapshot)
            .expect("saved USD document is restored");

        UsdSessionCodec.apply_view_state(app.world_mut(), id, &snapshot);

        let mut viewport = app.world_mut().resource_mut::<UsdViewportState>();
        assert!(viewport.preview_was_explicitly_closed(DocumentId::new(id)));
        assert!(viewport.take_auto_preview_suppression(DocumentId::new(id)));
        assert!(!viewport.take_auto_preview_suppression(DocumentId::new(id)));
    }

    #[test]
    fn captures_usd_preview_as_a_restorable_dock_tab() {
        let mut app = App::new();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        app.init_resource::<UsdViewportState>();
        let doc = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .restore(
                "#usda 1.0\n".into(),
                DocumentOrigin::File {
                    path: std::path::PathBuf::from("/tmp/session-test/scene.usda"),
                    writable: true,
                },
            );
        let preview = UsdPreviewId::for_document(doc);
        let view = UsdPreviewViewId(17);
        app.world_mut()
            .resource_mut::<UsdViewportState>()
            .insert(UsdPreviewSession::new(
                preview,
                doc,
                LayerId::root(),
                Entity::PLACEHOLDER,
                Handle::default(),
                1,
                view,
            ));
        assert!(
            app.world_mut()
                .resource_mut::<UsdViewportState>()
                .insert_view(UsdPreviewView::new(
                    view,
                    preview,
                    Entity::PLACEHOLDER,
                    Entity::PLACEHOLDER,
                    Entity::PLACEHOLDER,
                ))
                .is_ok()
        );
        app.world_mut()
            .resource_mut::<UsdViewportState>()
            .view_mut(view)
            .expect("preview view was inserted")
            .orbit
            .yaw = 1.0;

        let snapshot = UsdSessionCodec
            .capture(app.world_mut())
            .into_iter()
            .find(|(id, _)| *id == doc.raw())
            .map(|(_, snapshot)| snapshot)
            .expect("USD document is captured");

        assert_eq!(snapshot.tab_instance, view.0);
        assert_eq!(snapshot.view_state["edit_target"], "@root@");
        assert_eq!(snapshot.view_state["views"][0]["settings"]["yaw"], 1.0);
    }

    #[test]
    fn view_changes_invalidate_the_workspace_snapshot() {
        let mut app = App::new();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        app.init_resource::<UsdViewportState>();
        let doc = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .restore(
                "#usda 1.0\n".into(),
                DocumentOrigin::File {
                    path: std::path::PathBuf::from("/tmp/session-test/scene.usda"),
                    writable: true,
                },
            );
        let preview = UsdPreviewId::for_document(doc);
        let view = UsdPreviewViewId(17);
        let mut state = app.world_mut().resource_mut::<UsdViewportState>();
        state.insert(UsdPreviewSession::new(
            preview,
            doc,
            LayerId::root(),
            Entity::PLACEHOLDER,
            Handle::default(),
            1,
            view,
        ));
        assert!(
            state
                .insert_view(UsdPreviewView::new(
                    view,
                    preview,
                    Entity::PLACEHOLDER,
                    Entity::PLACEHOLDER,
                    Entity::PLACEHOLDER,
                ))
                .is_ok()
        );

        let clean = UsdSessionCodec.revision(app.world());
        app.world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .host_mut(doc)
            .expect("USD document exists")
            .document_mut()
            .mark_restored_dirty();
        let dirty = UsdSessionCodec.revision(app.world());
        assert_ne!(clean, dirty);

        app.world_mut()
            .resource_mut::<UsdViewportState>()
            .session_mut(preview)
            .expect("preview session exists")
            .edit_target = LayerId::runtime();
        let edit_target_changed = UsdSessionCodec.revision(app.world());
        assert_ne!(dirty, edit_target_changed);

        app.world_mut()
            .resource_mut::<UsdViewportState>()
            .view_mut(view)
            .expect("preview view exists")
            .orbit
            .yaw += 0.1;
        assert_ne!(edit_target_changed, UsdSessionCodec.revision(app.world()));
    }
}
