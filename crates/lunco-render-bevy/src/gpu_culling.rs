//! Use Bevy's GPU culler for scene-camera views when the adapter supports it.
//!
//! Bevy's camera-level `NoCpuCulling` moves camera frustum tests to GPU
//! preprocessing while leaving mesh-level light and shadow visibility intact.
//! GPU culling requires an active native GPU instance builder, device culling
//! support and an indirect-drawing camera. Other views retain CPU culling.
//! Render-world eligibility is shared with the main world; structural observers
//! reconcile view changes without per-frame entity scans.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use bevy::{
    camera::visibility::NoCpuCulling,
    pbr::RenderMeshInstances,
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        batching::gpu_preprocessing::{GpuPreprocessingMode, GpuPreprocessingSupport},
        view::NoIndirectDrawing,
    },
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
        .add_observer(on_camera_draw_configuration_added)
        .add_observer(on_camera_draw_configuration_removed)
        .add_systems(PreUpdate, reconcile_existing_scene_cameras);
}

fn publish_gpu_culling_capability(
    mut support: Option<ResMut<GpuPreprocessingSupport>>,
    capability: Res<GpuCullingCapability>,
    mesh_instances: Option<Res<RenderMeshInstances>>,
) {
    // RenderStartup refreshes device support first. Native view phases also
    // consume this resource, so their effective mode must match the active
    // instance builder; a CPU builder supplies no GPU preprocessing buffers.
    let gpu_builder = mesh_instances
        .as_deref()
        .is_some_and(|instances| matches!(instances, RenderMeshInstances::GpuBuilding(_)));
    if mesh_instances
        .as_deref()
        .is_some_and(|instances| matches!(instances, RenderMeshInstances::CpuBuilding(_)))
        && let Some(support) = support.as_deref_mut()
    {
        support.max_supported_mode = GpuPreprocessingMode::None;
    }
    capability.0.store(
        gpu_builder && support.is_some_and(|support| support.is_culling_supported()),
        Ordering::Release,
    );
}

fn on_camera_draw_configuration_added(
    add: On<Add<(SceneCamera, NoIndirectDrawing)>>,
    mut commands: Commands,
) {
    let entity = add.entity;
    commands.queue(move |world: &mut World| reconcile_scene_camera(world, entity));
}

fn on_camera_draw_configuration_removed(
    remove: On<Remove<(SceneCamera, NoIndirectDrawing)>>,
    mut commands: Commands,
) {
    let entity = remove.entity;
    // Removal observers see the old component set. Read the final state after
    // the structural operation, including a possible same-frame replacement.
    commands.queue(move |world: &mut World| reconcile_scene_camera(world, entity));
}

/// Apply native draw eligibility without taking ownership of another caller's marker.
fn reconcile_scene_camera(world: &mut World, entity: Entity) {
    let supported = world.resource::<GpuCullingCapability>().is_supported();
    let Ok(mut camera) = world.get_entity_mut(entity) else {
        return;
    };
    let eligible =
        supported && camera.contains::<SceneCamera>() && !camera.contains::<NoIndirectDrawing>();
    let ours = camera.contains::<LunCoGpuCulledCamera>();
    if eligible {
        if !camera.contains::<NoCpuCulling>() {
            camera.insert((NoCpuCulling, LunCoGpuCulledCamera));
        }
    } else if ours {
        camera.remove::<(NoCpuCulling, LunCoGpuCulledCamera)>();
    }
}

fn reconcile_existing_scene_cameras(
    capability: Res<GpuCullingCapability>,
    scene_cameras: Query<Entity, Or<(With<SceneCamera>, With<LunCoGpuCulledCamera>)>>,
    mut applied: Local<Option<bool>>,
    mut commands: Commands,
) {
    let supported = capability.is_supported();
    let previous = *applied;
    if previous == Some(supported) {
        return;
    }
    *applied = Some(supported);
    if !supported && previous.is_none() {
        return;
    }
    for entity in &scene_cameras {
        commands.queue(move |world: &mut World| reconcile_scene_camera(world, entity));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_culling_requires_the_active_gpu_instance_builder() {
        let capability = GpuCullingCapability::default();
        let mut app = App::new();
        app.insert_resource(capability.clone())
            .insert_resource(GpuPreprocessingSupport {
                max_supported_mode: GpuPreprocessingMode::Culling,
            })
            .insert_resource(RenderMeshInstances::CpuBuilding(Default::default()))
            .add_systems(Update, publish_gpu_culling_capability);
        app.update();
        assert!(
            !capability.is_supported(),
            "device support cannot replace an inactive GPU builder"
        );
        assert!(
            matches!(
                app.world()
                    .resource::<GpuPreprocessingSupport>()
                    .max_supported_mode,
                GpuPreprocessingMode::None
            ),
            "native render phases must match the active CPU builder"
        );
        // RenderStartup supplies fresh device support before each publication.
        app.world_mut()
            .resource_mut::<GpuPreprocessingSupport>()
            .max_supported_mode = GpuPreprocessingMode::Culling;
        app.insert_resource(RenderMeshInstances::GpuBuilding(Default::default()));
        app.update();
        assert!(capability.is_supported());
        app.world_mut()
            .resource_mut::<GpuPreprocessingSupport>()
            .max_supported_mode = GpuPreprocessingMode::PreprocessingOnly;
        app.update();
        assert!(!capability.is_supported());
        app.world_mut()
            .resource_mut::<GpuPreprocessingSupport>()
            .max_supported_mode = GpuPreprocessingMode::Culling;
        app.world_mut().remove_resource::<RenderMeshInstances>();
        app.update();
        assert!(!capability.is_supported());
    }

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
        app.world_mut()
            .entity_mut(existing)
            .insert(NoIndirectDrawing);
        app.update();
        assert!(!app.world().entity(existing).contains::<NoCpuCulling>());
        assert!(
            !app.world()
                .entity(existing)
                .contains::<LunCoGpuCulledCamera>()
        );
        app.world_mut()
            .entity_mut(existing)
            .remove::<NoIndirectDrawing>();
        app.update();
        assert!(app.world().entity(existing).contains::<NoCpuCulling>());
        app.world_mut()
            .entity_mut(authored)
            .insert(NoIndirectDrawing);
        app.update();
        assert!(app.world().entity(authored).contains::<NoCpuCulling>());

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
