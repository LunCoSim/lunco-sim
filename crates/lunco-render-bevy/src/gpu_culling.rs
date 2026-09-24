//! Use Bevy's GPU culler for scene-camera views when the adapter supports it.
//!
//! Bevy's camera-level `NoCpuCulling` moves camera frustum tests to GPU
//! preprocessing while leaving mesh-level light and shadow visibility intact.
//! This adapter is capability-gated: unsupported devices keep the default CPU
//! path, and the render-world capability is shared back to the main world
//! without per-frame entity scans.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use bevy::{
    camera::visibility::NoCpuCulling,
    prelude::*,
    render::{RenderApp, RenderStartup, batching::gpu_preprocessing::GpuPreprocessingSupport},
};
use lunco_render::camera::SceneCamera;

#[derive(Resource, Clone)]
struct GpuCullingCapability(Arc<AtomicBool>);

impl Default for GpuCullingCapability {
    fn default() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
}

impl GpuCullingCapability {
    fn is_supported(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Identifies only markers installed by this adapter, so a device recovery can
/// restore the CPU path without removing another owner's `NoCpuCulling`.
#[derive(Component)]
struct LunCoGpuCulledCamera;

pub(super) fn build(app: &mut App) {
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }

    let capability = GpuCullingCapability::default();
    install_main_world(app, capability.clone());
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(capability).add_systems(
            RenderStartup,
            publish_gpu_culling_capability
                .after(bevy::render::init_gpu_resource::<GpuPreprocessingSupport>),
        );
    }
}

fn install_main_world(app: &mut App, capability: GpuCullingCapability) {
    app.insert_resource(capability)
        .add_observer(opt_in_scene_camera)
        .add_observer(remove_owned_marker_with_scene_camera)
        .add_systems(PreUpdate, reconcile_existing_scene_cameras);
}

fn publish_gpu_culling_capability(
    support: Option<Res<GpuPreprocessingSupport>>,
    capability: Res<GpuCullingCapability>,
) {
    capability.0.store(
        support.is_some_and(|support| support.is_culling_supported()),
        Ordering::Release,
    );
}

fn opt_in_scene_camera(
    add: On<Add, SceneCamera>,
    capability: Res<GpuCullingCapability>,
    existing: Query<(Has<NoCpuCulling>, Has<LunCoGpuCulledCamera>)>,
    mut commands: Commands,
) {
    let entity = add.entity;
    let Ok((has_no_cpu_culling, is_ours)) = existing.get(entity) else {
        return;
    };

    if capability.is_supported() {
        if is_ours && !has_no_cpu_culling {
            commands.entity(entity).try_insert(NoCpuCulling);
        } else if !is_ours && !has_no_cpu_culling {
            commands
                .entity(entity)
                .try_insert((NoCpuCulling, LunCoGpuCulledCamera));
        }
    } else if is_ours {
        commands
            .entity(entity)
            .try_remove::<NoCpuCulling>()
            .try_remove::<LunCoGpuCulledCamera>();
    }
}

fn remove_owned_marker_with_scene_camera(
    remove: On<Remove, SceneCamera>,
    owned: Query<(), With<LunCoGpuCulledCamera>>,
    mut commands: Commands,
) {
    let entity = remove.entity;
    if owned.get(entity).is_ok() {
        commands
            .entity(entity)
            .try_remove::<NoCpuCulling>()
            .try_remove::<LunCoGpuCulledCamera>();
    }
}

fn reconcile_existing_scene_cameras(
    capability: Res<GpuCullingCapability>,
    scene_cameras: Query<
        (
            Entity,
            Has<SceneCamera>,
            Has<NoCpuCulling>,
            Has<LunCoGpuCulledCamera>,
        ),
        Or<(With<SceneCamera>, With<LunCoGpuCulledCamera>)>,
    >,
    mut applied: Local<Option<bool>>,
    mut commands: Commands,
) {
    let supported = capability.is_supported();
    let previous = *applied;
    if previous == Some(supported) {
        return;
    }
    *applied = Some(supported);

    // CPU culling is already the safe initial path. Avoid scanning all cameras
    // at startup on devices that cannot use GPU culling.
    if !supported && previous.is_none() {
        return;
    }

    for (entity, has_scene_camera, has_no_cpu_culling, is_ours) in &scene_cameras {
        if supported {
            if !has_scene_camera {
                continue;
            }
            if is_ours {
                if !has_no_cpu_culling {
                    commands.entity(entity).try_insert(NoCpuCulling);
                }
            } else if !has_no_cpu_culling {
                commands
                    .entity(entity)
                    .try_insert((NoCpuCulling, LunCoGpuCulledCamera));
            }
        } else if is_ours {
            commands
                .entity(entity)
                .try_remove::<NoCpuCulling>()
                .try_remove::<LunCoGpuCulledCamera>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh() -> Mesh3d {
        Mesh3d(Handle::<bevy::mesh::Mesh>::default())
    }

    fn scene_camera() -> (Camera, SceneCamera) {
        (Camera::default(), SceneCamera::default())
    }

    #[test]
    fn gpu_culling_is_camera_scoped_and_capability_gated_for_scene_cameras() {
        let capability = GpuCullingCapability::default();
        let mut app = App::new();
        install_main_world(&mut app, capability.clone());

        let existing = app.world_mut().spawn(scene_camera()).id();
        let authored = app.world_mut().spawn((NoCpuCulling, scene_camera())).id();
        let application_camera = app.world_mut().spawn(Camera::default()).id();
        let shadow_caster = app.world_mut().spawn(mesh()).id();
        app.update();
        assert!(!app.world().entity(existing).contains::<NoCpuCulling>());
        assert!(
            !app.world()
                .entity(application_camera)
                .contains::<NoCpuCulling>()
        );

        capability.0.store(true, Ordering::Release);
        app.update();
        assert!(app.world().entity(existing).contains::<NoCpuCulling>());
        assert!(
            app.world()
                .entity(existing)
                .contains::<LunCoGpuCulledCamera>()
        );
        assert!(
            !app.world()
                .entity(authored)
                .contains::<LunCoGpuCulledCamera>()
        );
        assert!(
            !app.world()
                .entity(application_camera)
                .contains::<NoCpuCulling>()
        );
        assert!(!app.world().entity(shadow_caster).contains::<NoCpuCulling>());

        let streamed = app.world_mut().spawn(scene_camera()).id();
        app.update();
        assert!(app.world().entity(streamed).contains::<NoCpuCulling>());
        assert!(
            app.world()
                .entity(streamed)
                .contains::<LunCoGpuCulledCamera>()
        );

        app.world_mut().entity_mut(streamed).remove::<SceneCamera>();
        app.update();
        assert!(!app.world().entity(streamed).contains::<NoCpuCulling>());
        assert!(
            !app.world()
                .entity(streamed)
                .contains::<LunCoGpuCulledCamera>()
        );

        capability.0.store(false, Ordering::Release);
        app.update();
        assert!(!app.world().entity(existing).contains::<NoCpuCulling>());
        assert!(
            !app.world()
                .entity(existing)
                .contains::<LunCoGpuCulledCamera>()
        );
        assert!(!app.world().entity(streamed).contains::<NoCpuCulling>());
        assert!(app.world().entity(authored).contains::<NoCpuCulling>());

        app.world_mut()
            .entity_mut(streamed)
            .insert(SceneCamera::default());
        app.update();
        assert!(!app.world().entity(streamed).contains::<NoCpuCulling>());
        assert!(
            !app.world()
                .entity(streamed)
                .contains::<LunCoGpuCulledCamera>()
        );
    }
}
