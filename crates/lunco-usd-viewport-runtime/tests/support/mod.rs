//! Shared helpers for document-level USD integration tests.

mod quality;
pub(crate) use quality::preview_quality;

use bevy::prelude::{App, Or, With};
use lunco_doc::Document;
use lunco_doc_bevy::DocumentRegistry;
use lunco_usd_bevy_scene::{UsdSceneAwaitingStage, UsdSceneProjectionQueued};
use lunco_usd_document::document::UsdDocument;
use lunco_usd_viewport_core::UsdViewportState;
use std::time::{Duration, Instant};

/// Wait for the exact open document generations and their visual projections,
/// including the dependent observer frame.
pub(crate) fn settle_visual_projection(app: &mut App) {
    let mut empty_frames = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.update();
        let current = {
            let state = app.world().resource::<UsdViewportState>();
            assert!(state.session_count() > 0, "preview open was rejected");
            let documents = app.world().resource::<DocumentRegistry<UsdDocument>>();
            state.sessions().all(|session| {
                let document = documents
                    .host(session.doc())
                    .expect("preview document is open");
                session.projection_ready()
                    && session.projected_generation() == document.document().generation()
            })
        };
        let pending = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<(), Or<(
                With<UsdSceneProjectionQueued>,
                With<UsdSceneAwaitingStage>,
            )>>();
            query.iter(world).next().is_some()
        };
        if pending || !current {
            empty_frames = 0;
        } else {
            empty_frames += 1;
            if empty_frames == 2 {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "USD visual projection did not settle within 10s"
        );
        std::thread::yield_now();
    }
}
