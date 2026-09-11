//! Render-free USD camera mechanisms for Bevy.
//!
//! This package owns authored camera projection, mounted and cinematic camera
//! pose, camera-track selection, viewport reconciliation, and generated
//! presentation contracts. It depends on the generic USD reader and scene
//! identity contracts, but never on the visual projection package. The visual
//! adapter installs [`UsdCameraPlugin`] and calls the camera projector directly;
//! consumers name these modules from this package rather than relying on a
//! facade in `lunco-usd-bevy`.

pub mod camera;
pub mod camera_mount;
pub mod camera_path;
pub mod camera_switch;
pub mod camera_track;

use bevy::prelude::{App, IntoScheduleConfigs, Plugin, PostUpdate, Update};

/// System set used by camera systems that consume the committed USD scene.
///
/// The visual adapter places this set after its structural projection set. The
/// camera package does not depend on that visual package, so the ordering edge
/// is declared by the integration root that owns both plugins.
#[derive(bevy::ecs::schedule::SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct UsdCameraProjectionSet;

/// Installs the camera-side ECS resources, observers, commands, and systems.
pub struct UsdCameraPlugin;

impl Plugin for UsdCameraPlugin {
    fn build(&self, app: &mut App) {
        register_all_commands(app);

        app.init_resource::<lunco_core::SceneViewport>()
            .init_resource::<lunco_core::SceneMountState>()
            .init_resource::<lunco_core::TheLocalAvatar>()
            .init_resource::<camera_switch::ViewportCameraSelection>()
            .init_resource::<camera_switch::CameraSelectionStatus>()
            .init_resource::<camera_switch::CameraContractStatus>()
            .init_resource::<camera_switch::StandalonePresentationState>()
            .init_resource::<camera_switch::StandalonePresentationSettings>()
            .register_type::<camera_track::CameraTrack>()
            .configure_sets(Update, UsdCameraProjectionSet)
            .add_observer(camera_switch::on_activate_camera)
            .add_observer(camera_switch::on_request_local_avatar_view)
            .add_systems(
                Update,
                camera_switch::cycle_active_camera.in_set(UsdCameraProjectionSet),
            )
            .configure_sets(
                PostUpdate,
                (
                    lunco_core::SceneViewportSet::Publish,
                    lunco_core::SceneViewportSet::Reconcile,
                )
                    .chain()
                    .before(bevy::camera::CameraUpdateSystems),
            )
            .add_systems(
                PostUpdate,
                (
                    camera_switch::reconcile_scene_viewport
                        .in_set(lunco_core::SceneViewportSet::Reconcile)
                        .before(camera_switch::update_camera_origin),
                    camera_switch::update_camera_selection_status
                        .after(camera_switch::reconcile_scene_viewport)
                        .run_if(camera_switch::camera_selection_status_changed),
                ),
            )
            .add_systems(
                lunco_core::SceneTeardown,
                camera_switch::reset_camera_selection,
            )
            .add_systems(
                Update,
                (
                    camera_mount::resolve_camera_mounts,
                    camera_path::resolve_camera_paths,
                    camera_switch::ensure_standalone_presentation,
                    camera_track::bind_camera_tracks_to_preview,
                    camera_track::clear_camera_track_plans_on_stage_reload.run_if(
                        bevy::ecs::schedule::common_conditions::on_message::<
                            bevy::asset::AssetEvent<lunco_usd_bevy_core::UsdStageAsset>,
                        >,
                    ),
                    camera_track::plan_camera_tracks,
                    camera_switch::validate_authored_camera_contract.run_if(
                        lunco_core::gate::tracked(
                            "usd::camera_contract",
                            camera_switch::camera_contract_inputs_changed,
                        ),
                    ),
                    camera_track::sample_camera_tracks.after(lunco_time::DomainResolveSet),
                )
                    .chain()
                    .in_set(UsdCameraProjectionSet),
            )
            .add_systems(
                PostUpdate,
                (
                    camera_path::drive_camera_paths,
                    camera_path::apply_camera_paths,
                )
                    .chain()
                    .in_set(camera_path::CameraPathSet),
            )
            .configure_sets(
                PostUpdate,
                camera_path::CameraPathSet
                    .in_set(bevy::transform::TransformSystems::Propagate)
                    .before(big_space::prelude::BigSpaceSystems::RecenterLargeTransforms),
            )
            .add_systems(
                PostUpdate,
                camera_switch::update_camera_origin
                    .in_set(bevy::transform::TransformSystems::Propagate)
                    .after(camera_path::CameraPathSet)
                    .before(big_space::prelude::BigSpaceSystems::RecenterLargeTransforms),
            )
            .add_systems(
                PostUpdate,
                camera_mount::follow_mounted_cameras
                    .before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

lunco_core::register_commands!(
    camera_switch::on_set_active_camera,
    camera_switch::on_set_user_camera,
    camera_switch::on_observe_avatar,
    camera_switch::on_resume_camera_director,
    camera_path::camera_path_transport,
);
