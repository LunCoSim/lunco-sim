//! Application-level composition of the USD runtime projections.
//!
//! The individual visual, diagnostics, physics, simulation, and document
//! command crates remain independently usable. This package owns only the
//! convenience bundle used by a complete application, keeping that aggregate
//! dependency closure out of headless USD document consumers.

use bevy::prelude::{App, IntoScheduleConfigs, Plugin};

mod live_consume;
mod program_runtime;
mod runtime_persistence;
mod scene_runtime;
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
        app.init_resource::<lunco_core::SceneTransitionCoordinator>();
        app.init_resource::<lunco_usd_core::commands::EmptyViewportReason>();
        app.add_observer(scene_runtime::clear_scene_on_twin_closed);
        app.add_systems(
            lunco_core::SceneTeardown,
            twin_projection::reset_scene_projection_state,
        );
        app.add_observer(scene_runtime::open_usd_docs_on_twin_asset_mounted);
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
        app.add_observer(runtime_persistence::on_doc_opened_load_runtime);
        app.add_observer(runtime_persistence::on_doc_changed_save_runtime);

        app.init_resource::<twin_projection::PendingTwinDocs>();
        app.init_resource::<lunco_usd_bevy_twin::TwinProjectionWake>();
        app.add_message::<twin_projection::TwinProjectionSettle>();
        app.add_observer(twin_projection::wake_twin_projection_on_document_changed);
        app.init_resource::<live_consume::LiveTransformEditHints>();
        app.init_resource::<twin_projection::PendingRefSpawns>();
        app.init_resource::<twin_projection::PendingInstanceProjections>();
        app.init_resource::<live_consume::PendingStageProjections>();
        app.add_systems(
            bevy::prelude::PreUpdate,
            (
                twin_projection::settle_twin_overlays,
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
                .run_if(
                    bevy::ecs::schedule::common_conditions::resource_exists::<
                        bevy::asset::AssetServer,
                    >,
                )
                .run_if(
                    bevy::ecs::schedule::common_conditions::resource_exists::<
                        bevy::asset::Assets<lunco_usd_bevy_core::source::UsdSourceText>,
                    >,
                ),
        );
        scene_runtime::register_all_commands(app);
    }
}

/// Install the complete USD runtime projection stack.
pub struct UsdPlugins;

impl Plugin for UsdPlugins {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            lunco_usd_bevy::UsdVisualPlugin,
            lunco_usd_bevy_animation::UsdAnimationPlugin,
            lunco_usd_bevy_diagnostics::UsdDiagnosticsPlugin,
            lunco_usd_avian::UsdAvianPlugin,
            lunco_usd_sim::UsdSimPlugin,
            lunco_usd_sim_cosim::UsdSimCosimPlugin,
        ));
        #[cfg(feature = "api")]
        app.add_plugins(lunco_usd_sim_cosim_api::UsdSimCosimApiPlugin);
        #[cfg(feature = "api")]
        app.add_plugins(lunco_usd_sim_domain_api::UsdSimDomainApiPlugin);
        app.add_plugins((lunco_usd_commands::UsdCommandsPlugin, UsdSceneRuntimePlugin));
    }
}
