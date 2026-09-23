//! Headless-safe USD scene admission and live projection runtime.
//!
//! This package owns the scene/Twin lifecycle, live document-to-stage
//! projection, and generic USD runtime consumption. Twin-scoped runtime-overlay
//! persistence is composed from `lunco-usd-bevy-runtime-persistence`.
//! Authored controls and executable programs are installed here after visual
//! scene admission, so the visual projector remains a reusable presentation
//! adapter. The Bevy scene-property port backend is a separate package and is
//! composed explicitly by the complete runtime bundle.
//! It does not assemble the complete visual/physics/simulation plugin bundle;
//! that application convenience composition remains in
//! `lunco-usd-bevy-runtime`.
//! Specialized domain plugins register their in-place edit owners through
//! [`lunco_usd_bevy_core::live_edit::UsdLiveEditRegistry`], keeping this
//! generic runtime independent of those domain implementations.

use bevy::prelude::{App, IntoScheduleConfigs, Plugin};

mod live_consume;
pub mod scene;
mod scene_runtime;
mod schema_assets;
mod twin_projection;

/// Install the USD scene admission and live projection systems.
///
/// Document registration and typed authoring commands remain in
/// `lunco-usd-commands`; this plugin owns only the scene/Twin runtime edge.
/// Add [`lunco_usd_commands::UsdCommandsPlugin`] first so the document registry
/// and Twin-document claims used by the runtime are installed.
pub struct UsdSceneRuntimePlugin;

impl Plugin for UsdSceneRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            bevy::prelude::Update,
            (
                schema_assets::request_schema_assets,
                schema_assets::register_ready_schema_assets,
            )
                .chain(),
        );
        app.init_resource::<lunco_core::SceneTransitionCoordinator>();
        app.init_resource::<lunco_usd_core::commands::EmptyViewportReason>();
        app.add_message::<lunco_usd_bevy_scene::UsdSceneProjectionReset>();
        app.add_message::<lunco_usd_bevy_scene::UsdSceneInfoChanged>();
        scene::install_scene_lifecycle(app);
        app.add_observer(scene::on_scene_transition_intent);
        app.add_observer(scene::execute_admitted_restart_scene);
        app.add_observer(scene::execute_admitted_clear_scene);
        app.add_observer(scene::on_scene_transition_completed);
        app.add_observer(scene::on_scene_transition_failed);
        app.add_observer(scene_runtime::clear_scene_on_twin_closed);
        app.add_systems(
            lunco_core::SceneTeardown,
            twin_projection::reset_scene_projection_state,
        );
        app.add_observer(scene_runtime::execute_admitted_load_scene);
        app.add_observer(scene_runtime::on_restart_scene_refresh_active_document);
        app.add_observer(
            |trigger: bevy::ecs::observer::On<lunco_core::SceneTransitionFailed>,
             mut empty_reason: bevy::ecs::system::ResMut<
                lunco_usd_core::commands::EmptyViewportReason,
            >| {
                let (lunco_core::SceneTransition::Load { path, .. }
                | lunco_core::SceneTransition::Restart { path, .. }) = &trigger.event().transition
                else {
                    return;
                };
                empty_reason.0 = Some(format!(
                    "`{path}` could not be loaded: {}",
                    trigger.event().error
                ));
            },
        );
        app.add_plugins(lunco_usd_bevy_runtime_persistence::UsdRuntimePersistencePlugin);

        app.init_resource::<twin_projection::PendingTwinDocs>();
        app.init_resource::<lunco_usd_bevy_twin::TwinProjectionWake>();
        app.add_observer(twin_projection::wake_twin_projection_on_document_changed);
        app.init_resource::<live_consume::LiveTransformEditHints>();
        app.init_resource::<twin_projection::PendingRefSpawns>();
        app.init_resource::<twin_projection::PendingInstanceProjections>();
        app.init_resource::<live_consume::PendingStageProjections>();
        app.add_systems(
            bevy::prelude::PreUpdate,
            (
                twin_projection::mark_pending_twin_docs,
                twin_projection::drain_pending_twin_docs
                    .run_if(twin_projection::pending_twin_docs_ready),
                twin_projection::wake_twin_projection_on_stage_event,
                twin_projection::sync_twin_overlays.run_if(twin_projection::twin_projection_ready),
                twin_projection::mark_pending_ref_spawns,
                twin_projection::sync_stage_dependency_diagnostics,
                twin_projection::drain_ref_spawns.run_if(twin_projection::pending_ref_spawns_ready),
                live_consume::project_stage_changes,
            )
                .chain()
                .in_set(lunco_core::RuntimeCycleSet::Lifecycle)
                .run_if(
                    bevy::ecs::schedule::common_conditions::resource_exists::<
                        bevy::asset::AssetServer,
                    >,
                )
                .run_if(
                    bevy::ecs::schedule::common_conditions::resource_exists::<
                        bevy::asset::Assets<lunco_usd_bevy_stage::source::UsdSourceText>,
                    >,
                ),
        );
        app.add_plugins(lunco_usd_bevy_authored_runtime::UsdAuthoredRuntimePlugin);
        scene::register_all_commands(app);
        scene_runtime::register_all_commands(app);
    }
}
