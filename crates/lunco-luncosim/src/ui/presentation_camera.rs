//! Windowed presentation policy for scenes without an authored viewport camera.
//!
//! A scene camera is normally an authored USD fact. A camera-less scene is
//! still loadable, however, and leaving the window blank makes a successful
//! load look like a hang. This module supplies one explicitly engine-owned
//! presentation camera after scene projection has settled. It publishes that
//! ownership through the shared camera-selection state, so the Camera menu and
//! status bus explain the choice. The camera is retired by the camera-selection
//! authority as soon as an authored camera appears.

use bevy::prelude::*;
use lunco_render::{GraphicsCameraDefaults, SceneCamera};
use lunco_usd_bevy::{camera_switch, UsdAwaitingStage, UsdPrimPath};
use lunco_workbench::status_bus::{StatusBus, StatusLevel, CAMERA_SOURCE};

const FALLBACK_CAMERA_NAME: &str = "LunCo presentation view";

/// Tunable framing for the explicit camera-less-scene presentation policy.
/// These values are a semantic default for an omitted viewport camera, not an
/// authored scene fact or a replacement for a malformed authored camera.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub(crate) struct PresentationFallbackCameraSettings {
    pub position: Vec3,
    pub look_at: Vec3,
}

impl Default for PresentationFallbackCameraSettings {
    fn default() -> Self {
        Self {
            position: Vec3::new(-30.0, 15.0, -20.0),
            look_at: Vec3::ZERO,
        }
    }
}

/// Create one explicit presentation camera only after the scene's stage and
/// prim projection have settled. Waiting for those lifecycle signals prevents
/// the policy from racing an authored camera that is still being projected.
pub(crate) fn spawn_presentation_fallback_camera(
    mut commands: Commands,
    settings: Res<PresentationFallbackCameraSettings>,
    in_flight: Option<Res<lunco_usd_sim::cosim::SceneLoadInFlight>>,
    awaiting: Query<(), With<UsdAwaitingStage>>,
    prims: Query<(Entity, &UsdPrimPath)>,
    authored: Query<
        (),
        (
            With<SceneCamera>,
            Without<camera_switch::PresentationFallbackCamera>,
        ),
    >,
    existing: Query<(), With<camera_switch::PresentationFallbackCamera>>,
) {
    if in_flight.is_some() || !awaiting.is_empty() || !authored.is_empty() || !existing.is_empty() {
        return;
    }

    let Some((root, _)) = prims
        .iter()
        .min_by_key(|(_, path)| path.path.matches('/').count())
    else {
        return;
    };

    commands.spawn((
        (
            SceneCamera::agx(),
            GraphicsCameraDefaults,
            lunco_render::usd_default_perspective_projection(),
        ),
        Name::new(FALLBACK_CAMERA_NAME),
        Transform::from_translation(settings.position).looking_at(settings.look_at, Vec3::Y),
        camera_switch::PresentationFallbackCamera,
        ChildOf(root),
    ));
}

/// Route fallback activation through the shared typed camera-selection event.
/// The camera switch remains the only writer of viewport selection intent and
/// the only owner of active-camera reconciliation.
pub(crate) fn activate_presentation_fallback(
    fallback: Query<Entity, With<camera_switch::PresentationFallbackCamera>>,
    authored: Query<
        (),
        (
            With<SceneCamera>,
            Without<camera_switch::PresentationFallbackCamera>,
        ),
    >,
    selection: Res<camera_switch::ViewportCameraSelection>,
    mut commands: Commands,
) {
    if !authored.is_empty() || selection.owner() != camera_switch::CameraSelectionOwner::None {
        return;
    }
    let Some(target) = fallback.iter().next() else {
        return;
    };
    commands.trigger(camera_switch::ActivateCamera::fallback(target));
}

/// Make the exceptional camera policy visible in the same status history used
/// by terrain, scene loading, and dataset provisioning. This is edge-triggered
/// so a steady fallback view does not spam the event log every frame.
pub(crate) fn report_presentation_fallback(
    status: Res<camera_switch::CameraSelectionStatus>,
    bus: Option<ResMut<StatusBus>>,
    mut previous: Local<Option<camera_switch::CameraSelectionOwner>>,
) {
    let Some(mut bus) = bus else { return };
    if *previous == Some(status.owner) {
        return;
    }
    *previous = Some(status.owner);
    if status.owner == camera_switch::CameraSelectionOwner::Fallback {
        bus.push(
            CAMERA_SOURCE,
            StatusLevel::Warn,
            "No authored viewport camera; using the default presentation view. Open Camera to choose an authored view.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_is_spawned_only_after_scene_projection_settles() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(PresentationFallbackCameraSettings::default())
            .init_resource::<camera_switch::ViewportCameraSelection>()
            .add_systems(Update, spawn_presentation_fallback_camera);
        let root = app
            .world_mut()
            .spawn(UsdPrimPath {
                stage_handle: Default::default(),
                path: "/Luna2".into(),
            })
            .id();
        app.world_mut().spawn((
            UsdPrimPath {
                stage_handle: Default::default(),
                path: "/Luna2/Terrain".into(),
            },
            ChildOf(root),
        ));

        app.world_mut()
            .insert_resource(lunco_usd_sim::cosim::SceneLoadInFlight {
                path: "scene.usda".into(),
                stage_id: Default::default(),
            });
        app.update();
        assert!(app
            .world_mut()
            .query_filtered::<(), With<camera_switch::PresentationFallbackCamera>>()
            .iter(app.world())
            .next()
            .is_none());

        app.world_mut()
            .remove_resource::<lunco_usd_sim::cosim::SceneLoadInFlight>();
        app.update();
        assert_eq!(
            app.world_mut()
                .query_filtered::<(), With<camera_switch::PresentationFallbackCamera>>()
                .iter(app.world())
                .count(),
            1
        );
    }
}
