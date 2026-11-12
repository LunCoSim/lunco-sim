//! Binds [`SceneCamera`] intent to a real render pipeline.
//!
//! Domain crates spawn `Camera` + `SceneCamera` (both render-free) and filter on
//! `With<SceneCamera>`. This module attaches the `bevy_core_pipeline` half —
//! `Camera3d`, tonemapping, MSAA, bloom — which is what actually costs wgpu.
//!
//! See `lunco_render::camera` for why, and for the two `R4` bugs this closes.

use bevy::camera::{ClearColorConfig, Hdr};
use bevy::core_pipeline::prepass::{DepthPrepass, MotionVectorPrepass, NormalPrepass};
use bevy::light::{ClusteredDecal, LightProbe, PointLight, SpotLight, cluster::ClusterConfig};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::view::Tonemapping;
use lunco_render::camera::{
    CameraRetiring, MsaaLevel, SceneCamera, SceneViewDemand, SceneViewDemandSet, SceneViewLayers,
    SceneViewOutput, ToneMap,
};

use crate::RenderProfile;

pub(crate) fn build(app: &mut App) {
    let initial_clusterable_object_count = count_clusterable_objects(app.world_mut());
    app.init_resource::<RenderProfile>()
        .configure_sets(
            PostUpdate,
            SceneViewDemandSet::Publish.after(TransformSystems::Propagate),
        )
        // Native camera passes use the existing typed component authoring path.
        .register_type::<DepthPrepass>()
        .register_type::<NormalPrepass>()
        .register_type::<MotionVectorPrepass>()
        .insert_resource(ClusterableObjectCount(initial_clusterable_object_count))
        .init_resource::<lunco_render::RenderingQualitySettings>()
        .init_resource::<lunco_render::SceneBloomOverride>()
        .add_observer(configure_added_scene_camera)
        .add_observer(bind_scene_camera)
        .add_observer(bind_camera_feed)
        .add_observer(bind_camera_cluster_config)
        .add_observer(track_clusterable_object_added::<PointLight>)
        .add_observer(track_clusterable_object_added::<SpotLight>)
        .add_observer(track_clusterable_object_added::<LightProbe>)
        .add_observer(track_clusterable_object_added::<ClusteredDecal>)
        .add_observer(track_clusterable_object_removed::<PointLight>)
        .add_observer(track_clusterable_object_removed::<SpotLight>)
        .add_observer(track_clusterable_object_removed::<LightProbe>)
        .add_observer(track_clusterable_object_removed::<ClusteredDecal>)
        .add_systems(
            Update,
            (
                sync_new_scene_camera,
                sync_auto_cluster_configs.run_if(resource_changed::<ClusterableObjectCount>),
                apply_graphics_camera_quality.run_if(
                    resource_changed::<lunco_render::RenderingQualitySettings>
                        .or_else(resource_changed::<lunco_render::SceneBloomOverride>),
                ),
                rebind_changed_scene_camera,
            )
                .chain(),
        )
        .add_systems(
            PostUpdate,
            (
                publish_main_view_demand,
                retire_removed_scene_view_demands,
                publish_scene_view_layers,
                publish_default_shadow_origin,
            )
                .chain()
                .in_set(SceneViewDemandSet::Publish)
                .after(lunco_viewport_core::SceneViewportSet::Reconcile)
                .after(bevy::camera::CameraUpdateSystems)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
}

#[derive(Component)]
struct SelectedOutputShadowOrigin;

/// An explicit authored origin takes precedence. Otherwise the selected main
/// output supplies the application's shared point/spot shadow sampling origin.
fn publish_default_shadow_origin(
    outputs: Query<(Entity, &SceneViewDemand), With<SceneViewLayers>>,
    origins: Query<(Entity, Has<SelectedOutputShadowOrigin>), With<bevy::camera::ShadowLodOrigin>>,
    mut commands: Commands,
) {
    let authored = origins.iter().any(|(_, owned)| !owned);
    let selected = (!authored)
        .then(|| {
            outputs
                .iter()
                .find(|(_, demand)| demand.output == SceneViewOutput::MainViewport)
                .map(|(entity, _)| entity)
        })
        .flatten();
    for (entity, owned) in &origins {
        if owned && selected != Some(entity) {
            commands
                .entity(entity)
                .remove::<(bevy::camera::ShadowLodOrigin, SelectedOutputShadowOrigin)>();
        }
    }
    if let Some(entity) = selected {
        if origins.get(entity).is_err() {
            commands
                .entity(entity)
                .insert((bevy::camera::ShadowLodOrigin, SelectedOutputShadowOrigin));
        }
    }
}

fn publish_main_view_demand(
    viewport: Option<Res<lunco_viewport_core::SceneViewport>>,
    cameras: Query<(Entity, &Camera, Option<&SceneViewDemand>), With<SceneCamera>>,
    mut commands: Commands,
) {
    for (entity, camera, demand) in &cameras {
        let selected = viewport
            .as_ref()
            .is_some_and(|v| v.visible && v.active_camera == Some(entity))
            && camera.is_active
            && matches!(
                camera.output_mode,
                bevy::camera::CameraOutputMode::Write { .. }
            );
        if selected {
            let next = SceneViewDemand {
                source: entity,
                output: SceneViewOutput::MainViewport,
            };
            if demand != Some(&next) {
                commands.entity(entity).insert(next);
            }
        } else if demand.is_some_and(|d| d.output == SceneViewOutput::MainViewport) {
            commands.entity(entity).remove::<SceneViewDemand>();
        }
    }
}

#[derive(Default)]
struct SceneViewLayerAllocator {
    frame: u64,
    leases: std::collections::BTreeMap<Entity, (SceneViewDemand, usize)>,
    retired: Vec<(u64, usize)>,
    failed: std::collections::HashSet<Entity>,
}

fn retire_removed_scene_view_demands(
    mut removed: RemovedComponents<SceneViewDemand>,
    mut views: Query<(
        &mut bevy::camera::visibility::RenderLayers,
        &SceneViewLayers,
    )>,
    mut commands: Commands,
) {
    for entity in removed.read() {
        if let Ok((mut layers, lease)) = views.get_mut(entity) {
            let base = layers.clone().without(lease.layer);
            layers.set_if_neq(base);
            commands.entity(entity).remove::<SceneViewLayers>();
        }
    }
}

/// Four publication boundaries allow retired geometry's two extraction grace
/// frames to finish before another output can receive its channel.
fn publish_scene_view_layers(world: &mut World, mut allocator: Local<SceneViewLayerAllocator>) {
    use bevy::camera::visibility::RenderLayers;
    allocator.frame = allocator.frame.saturating_add(1);
    let frame = allocator.frame;
    allocator.retired.retain(|(ready, _)| *ready > frame);
    let mut query = world
        .query_filtered::<(Entity, &Camera, &SceneViewDemand, Has<CameraRetiring>), With<Camera3d>>(
        );
    let mut active = query
        .iter(world)
        .filter(|(_, camera, demand, retiring)| {
            !*retiring
                && camera.is_active
                && matches!(
                    camera.output_mode,
                    bevy::camera::CameraOutputMode::Write { .. }
                )
                && camera
                    .physical_viewport_size()
                    .is_some_and(|size| size.x > 0 && size.y > 0)
                && world.get_entity(demand.source).is_ok()
                && world.get::<CameraRetiring>(demand.source).is_none()
                && world
                    .get::<bevy::ecs::entity_disabling::Disabled>(demand.source)
                    .is_none()
        })
        .map(|(entity, _, demand, _)| (entity, *demand))
        .collect::<Vec<_>>();
    active.sort_unstable_by_key(|(entity, _)| *entity);
    allocator
        .failed
        .retain(|entity| active.iter().any(|(active, _)| active == entity));
    let retired = allocator
        .leases
        .iter()
        .filter_map(|(entity, (demand, _))| {
            (world.get::<SceneViewLayers>(*entity).is_none()
                || !active
                    .iter()
                    .any(|(active, current)| active == entity && current == demand))
            .then_some(*entity)
        })
        .collect::<Vec<_>>();
    for entity in retired {
        let (_, layer) = allocator.leases.remove(&entity).unwrap();
        allocator.retired.push((frame.saturating_add(4), layer));
        allocator.failed.remove(&entity);
        if let Ok(mut camera) = world.get_entity_mut(entity) {
            let layers = camera
                .get::<RenderLayers>()
                .cloned()
                .unwrap_or_default()
                .without(layer);
            camera.insert(layers).remove::<SceneViewLayers>();
        }
    }
    for (entity, demand) in active {
        let layer = if let Some((_, layer)) = allocator.leases.get(&entity).copied() {
            layer
        } else {
            let available = (64..320).find(|candidate| {
                !allocator
                    .leases
                    .values()
                    .any(|(_, layer)| layer == candidate)
                    && !allocator
                        .retired
                        .iter()
                        .any(|(_, layer)| layer == candidate)
            });
            let Some(layer) = available else {
                if allocator.failed.insert(entity) {
                    lunco_core::trigger_runtime_error(
                        &mut world.commands(),
                        "scene-view-layer-budget",
                        format!(
                            "render camera {entity:?} exceeds the 256 active output layer leases"
                        ),
                    );
                }
                continue;
            };
            allocator.leases.insert(entity, (demand, layer));
            allocator.failed.remove(&entity);
            layer
        };
        let mut camera = world.entity_mut(entity);
        let base = camera
            .get::<RenderLayers>()
            .cloned()
            .unwrap_or_default()
            .without(layer);
        let effective = base.clone().with(layer);
        if camera.get::<RenderLayers>() != Some(&effective) {
            camera.insert(effective);
        }
        let next = SceneViewLayers { layer, base };
        if camera.get::<SceneViewLayers>() != Some(&next) {
            camera.insert(next);
        }
    }
}

fn tonemapping_of(t: ToneMap) -> Tonemapping {
    match t {
        ToneMap::Linear => Tonemapping::Linear,
        ToneMap::TonyMcMapface => Tonemapping::TonyMcMapface,
        ToneMap::AgX => Tonemapping::AgX,
        ToneMap::AcesFitted => Tonemapping::AcesFitted,
        ToneMap::Reinhard => Tonemapping::Reinhard,
    }
}

fn bind_camera_feed(
    add: On<Add<lunco_render::camera::CameraFeedCamera>>,
    feeds: Query<&lunco_render::camera::CameraFeedCamera>,
    profile: Res<RenderProfile>,
    mut commands: Commands,
) {
    if let Ok(feed) = feeds.get(add.entity) {
        apply(&mut commands, add.entity, &feed.0, *profile);
        commands.entity(add.entity).try_insert(Camera {
            order: -1,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        });
    }
}

/// Bevy's default camera configuration allocates a 4,096-cell clustered-light
/// grid for every `Camera3d`. Directional lights do not use that grid, and most
/// LunCoSim scenes have no clusterable objects at all. Keep the decision at the
/// render camera owner, but drive it from component lifecycle events so a light
/// added by a live USD projection receives the correct Bevy path automatically.
#[derive(Resource, Debug, Default, PartialEq, Eq)]
struct ClusterableObjectCount(usize);

/// Marks a `ClusterConfig` that the render camera binder owns. An explicit Bevy
/// `ClusterConfig` supplied by a caller is left untouched; only the binder's
/// automatically selected configuration follows the scene's light topology.
#[derive(Component, Debug, Clone, Copy, Default)]
struct AutoClusterConfig;

fn count_clusterable_objects(world: &mut World) -> usize {
    world
        .query_filtered::<(), With<PointLight>>()
        .iter(world)
        .count()
        + world
            .query_filtered::<(), With<SpotLight>>()
            .iter(world)
            .count()
        + world
            .query_filtered::<(), With<LightProbe>>()
            .iter(world)
            .count()
        + world
            .query_filtered::<(), With<ClusteredDecal>>()
            .iter(world)
            .count()
}

impl ClusterableObjectCount {
    fn add(&mut self) {
        self.0 = self
            .0
            .checked_add(1)
            .expect("clusterable object count overflow");
    }

    fn remove(&mut self) {
        self.0 = self
            .0
            .checked_sub(1)
            .expect("clusterable object removal without a matching addition");
    }

    fn config(&self) -> ClusterConfig {
        if self.0 == 0 {
            ClusterConfig::Single
        } else {
            ClusterConfig::default()
        }
    }
}

fn track_clusterable_object_added<T: Component>(
    _trigger: On<Add<T>>,
    mut count: ResMut<ClusterableObjectCount>,
) {
    count.add();
}

fn track_clusterable_object_removed<T: Component>(
    _trigger: On<Remove<T>>,
    mut count: ResMut<ClusterableObjectCount>,
) {
    count.remove();
}

fn sync_auto_cluster_configs(
    count: Res<ClusterableObjectCount>,
    mut cameras: Query<&mut ClusterConfig, With<AutoClusterConfig>>,
) {
    for mut config in &mut cameras {
        let desired = count.config();
        match (&mut *config, desired) {
            (current, ClusterConfig::Single) if !matches!(current, ClusterConfig::Single) => {
                *current = ClusterConfig::Single;
            }
            (current, ClusterConfig::FixedZ { .. }) if matches!(current, ClusterConfig::Single) => {
                *current = desired;
            }
            _ => {}
        }
    }
}

fn bind_camera_cluster_config(
    add: On<Add<Camera3d>>,
    existing_cluster_configs: Query<(), With<ClusterConfig>>,
    clusterable_objects: Res<ClusterableObjectCount>,
    mut commands: Commands,
) {
    let e = add.entity;
    if existing_cluster_configs.get(e).is_ok() {
        return;
    }
    commands
        .entity(e)
        .try_insert((clusterable_objects.config(), AutoClusterConfig));
}

fn msaa_of(m: MsaaLevel) -> Msaa {
    match m {
        MsaaLevel::Off => Msaa::Off,
        MsaaLevel::X2 => Msaa::Sample2,
        MsaaLevel::X4 => Msaa::Sample4,
    }
}

/// Attach the pipeline components a `SceneCamera` describes.
fn apply(commands: &mut Commands, e: Entity, cam: &SceneCamera, profile: RenderProfile) {
    let mut ec = commands.entity(e);
    let (tonemapping, msaa) = if profile.is_fast() {
        (Tonemapping::Linear, Msaa::Off)
    } else {
        (tonemapping_of(cam.tone_map), msaa_of(cam.msaa))
    };
    ec.try_insert((Camera3d::default(), tonemapping, msaa));

    // SPACE IS BLACK. The global `ClearColor` is the WINDOW's colour and is set to the
    // workbench panel fill (`0x1a1a1a`) so the chrome has no seam; a `Camera3d` that
    // inherits it renders the vacuum as that same grey. That is not cosmetic: the
    // the starfield is an emissive backdrop written into the opaque scene pass, and
    // 0x1a1a1a is ~0.01 linear — enough to swamp its dim stars (measured: sky region
    // mean 26/255, σ 0.4 — a flat grey with the stars invisible). Clearing the SCENE
    // camera to black leaves the chrome's own clear untouched and gives the authored
    // background its intended black space.
    //
    // `Hdr` is a marker component in `bevy_camera` — render-FREE. So "this camera is
    // HDR" is expressible headless too; only the pipeline that acts on it is not.
    if cam.hdr && !profile.is_fast() {
        ec.try_insert(Hdr);
    } else {
        ec.try_remove::<Hdr>();
    }

    match (cam.bloom, cam.hdr, profile) {
        (_, _, RenderProfile::Fast) => {
            ec.try_remove::<Bloom>();
        }
        (Some(b), true, _) => {
            ec.try_insert(Bloom {
                intensity: b.intensity,
                low_frequency_boost: b.low_frequency_boost,
                ..Bloom::default()
            });
        }
        (Some(_), false, _) => {
            warn!(
                "SceneCamera on {e:?} asks for bloom without hdr — refusing. Bloom on a \
                 non-HDR target renders nothing and still pays for the downsample chain. \
                 Use `SceneCamera::with_bloom`, which turns hdr on for you."
            );
            ec.try_remove::<Bloom>();
        }
        (None, _, _) => {
            ec.try_remove::<Bloom>();
        }
    }
}

/// Apply the persisted Graphics camera settings to existing scene-camera intent.
///
/// Environment bloom is a USD-owned override in [`SceneBloomOverride`]. Its
/// intensity survives a Graphics edit while the renderer-owned bloom shape still
/// follows the current settings. Unauthored cameras are rebuilt from the same
/// profile used by new USD/avatar cameras, so the menu is live rather than a
/// startup-only write.
fn apply_graphics_camera_quality(
    settings: Res<lunco_render::RenderingQualitySettings>,
    bloom_override: Res<lunco_render::SceneBloomOverride>,
    mut cameras: Query<
        (&mut SceneCamera, Has<lunco_render::EditorPreviewCamera>),
        With<lunco_render::GraphicsCameraDefaults>,
    >,
) {
    let profile = match settings.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            warn!("invalid Graphics camera settings: {reason}; preserving current camera intent");
            return;
        }
    };
    for (mut camera, editor) in &mut cameras {
        apply_camera_quality(&mut camera, profile, bloom_override.intensity, editor);
    }
}

fn sync_new_scene_camera(
    settings: Res<lunco_render::RenderingQualitySettings>,
    bloom_override: Res<lunco_render::SceneBloomOverride>,
    mut cameras: Query<
        (&mut SceneCamera, Has<lunco_render::EditorPreviewCamera>),
        (
            With<lunco_render::GraphicsCameraDefaults>,
            Added<SceneCamera>,
        ),
    >,
) {
    let profile = match settings.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            warn!(
                "invalid Graphics camera settings: {reason}; new camera keeps its authored intent"
            );
            return;
        }
    };
    for (mut camera, editor) in &mut cameras {
        apply_camera_quality(&mut camera, profile, bloom_override.intensity, editor);
    }
}

fn apply_camera_quality(
    camera: &mut SceneCamera,
    profile: lunco_render::RenderQualityProfile,
    bloom_override: Option<f32>,
    editor: bool,
) {
    camera.tone_map = profile.camera_tone_map;
    camera.msaa = profile.camera_msaa;
    let bloom_intensity = if editor && !profile.editor_camera_bloom_enabled {
        0.0
    } else {
        bloom_override.unwrap_or(profile.camera_bloom_intensity)
    };
    camera.bloom = (bloom_intensity > 0.0).then(|| {
        lunco_render::BloomLook::new(bloom_intensity, profile.camera_bloom_low_frequency_boost)
    });
    camera.hdr = camera.bloom.is_some();
}

/// Configure the existing camera while it is still borrowed by this system.
/// Scene teardown is deferred, so a queued `EntityEntry` mutation can outlive
/// the camera and panic when the teardown buffer is applied. The viewport
/// reconciler owns `is_active`; this function only applies the camera's
/// render-clear intent.
fn configure_camera(camera: Option<&mut Camera>) {
    let Some(camera) = camera else { return };
    if !matches!(camera.clear_color, ClearColorConfig::Custom(_)) {
        camera.clear_color = ClearColorConfig::Custom(Color::BLACK);
    }
}

/// Configure the clear intent when `Camera3d` supplies a required `Camera`
/// component after the `SceneCamera` observer has already run.
fn configure_added_scene_camera(
    add: On<Add<Camera>>,
    scene_cameras: Query<(), With<SceneCamera>>,
    mut cameras: Query<&mut Camera>,
) {
    let entity = add.entity;
    if scene_cameras.get(entity).is_ok() {
        if let Ok(mut camera) = cameras.get_mut(entity) {
            configure_camera(Some(&mut camera));
        }
    }
}

fn bind_scene_camera(
    add: On<Add<SceneCamera>>,
    cams: Query<&SceneCamera>,
    mut cameras: Query<&mut Camera>,
    profile: Res<RenderProfile>,
    mut commands: Commands,
) {
    let e = add.entity;
    let Ok(cam) = cams.get(e) else { return };
    if let Ok(mut camera) = cameras.get_mut(e) {
        configure_camera(Some(&mut camera));
    }
    apply(&mut commands, e, cam, *profile);
}

/// Re-apply when the look is retuned live (the render-settings panel).
fn rebind_changed_scene_camera(
    mut changed: Query<(Entity, &SceneCamera, Option<&mut Camera>), Changed<SceneCamera>>,
    profile: Res<RenderProfile>,
    mut commands: Commands,
) {
    for (e, cam, mut camera) in &mut changed {
        configure_camera(camera.as_deref_mut());
        apply(&mut commands, e, cam, *profile);
    }
}

#[cfg(test)]
mod view_output_tests {
    use super::*;
    use bevy::camera::{Viewport, visibility::RenderLayers};

    fn output(world: &mut World, source: Entity, id: u64) -> Entity {
        world
            .spawn((
                Camera3d::default(),
                Camera {
                    viewport: Some(Viewport {
                        physical_size: UVec2::new(640, 360),
                        ..default()
                    }),
                    ..default()
                },
                SceneViewDemand {
                    source,
                    output: SceneViewOutput::Observation { id },
                },
                RenderLayers::layer(3),
            ))
            .id()
    }

    #[test]
    fn active_outputs_isolate_geometry_and_release_only_their_owned_layer() {
        let mut app = App::new();
        app.add_systems(
            Update,
            (retire_removed_scene_view_demands, publish_scene_view_layers).chain(),
        );
        let source = app.world_mut().spawn_empty().id();
        let first = output(app.world_mut(), source, 1);
        let second = output(app.world_mut(), source, 2);
        app.update();
        let first_layer = app.world().get::<SceneViewLayers>(first).unwrap().layer;
        let second_layer = app.world().get::<SceneViewLayers>(second).unwrap().layer;
        assert_ne!(first_layer, second_layer);
        assert!(
            !app.world()
                .get::<RenderLayers>(first)
                .unwrap()
                .iter()
                .any(|n| n == second_layer)
        );
        app.world_mut()
            .entity_mut(first)
            .insert(RenderLayers::layer(9).with(first_layer));
        app.update();
        assert_eq!(
            app.world().get::<SceneViewLayers>(first).unwrap().base,
            RenderLayers::layer(9)
        );
        app.world_mut().get_mut::<Camera>(first).unwrap().is_active = false;
        app.update();
        assert!(app.world().get::<SceneViewLayers>(first).is_none());
        assert_eq!(
            app.world().get::<RenderLayers>(first),
            Some(&RenderLayers::layer(9))
        );
        assert_eq!(
            app.world().get::<SceneViewLayers>(second).unwrap().layer,
            second_layer
        );
        app.world_mut().get_mut::<Camera>(first).unwrap().is_active = true;
        app.update();
        assert_ne!(
            app.world().get::<SceneViewLayers>(first).unwrap().layer,
            first_layer,
            "retired geometry cannot enter a newly admitted output during its extraction grace frames"
        );
    }

    #[test]
    fn replacing_output_or_source_retires_the_exact_lease_and_disabled_sources_release_it() {
        let mut app = App::new();
        app.add_systems(
            Update,
            (retire_removed_scene_view_demands, publish_scene_view_layers).chain(),
        );
        let source = app.world_mut().spawn_empty().id();
        let camera = output(app.world_mut(), source, 1);
        app.update();
        let first = app.world().get::<SceneViewLayers>(camera).unwrap().layer;
        app.world_mut()
            .get_mut::<SceneViewDemand>(camera)
            .unwrap()
            .output = SceneViewOutput::Observation { id: 2 };
        app.update();
        let second = app.world().get::<SceneViewLayers>(camera).unwrap().layer;
        assert_ne!(first, second);
        let demand = *app.world().get::<SceneViewDemand>(camera).unwrap();
        app.world_mut()
            .entity_mut(camera)
            .remove::<SceneViewDemand>()
            .insert(demand);
        app.update();
        let readded = app.world().get::<SceneViewLayers>(camera).unwrap().layer;
        assert_ne!(
            second, readded,
            "remove/readd invalidates an identical output lease"
        );
        let replacement = app.world_mut().spawn_empty().id();
        app.world_mut()
            .get_mut::<SceneViewDemand>(camera)
            .unwrap()
            .source = replacement;
        app.update();
        assert_ne!(
            app.world().get::<SceneViewLayers>(camera).unwrap().layer,
            readded
        );
        app.world_mut()
            .entity_mut(replacement)
            .insert(bevy::ecs::entity_disabling::Disabled);
        app.update();
        assert!(app.world().get::<SceneViewLayers>(camera).is_none());
        app.world_mut()
            .entity_mut(replacement)
            .remove::<bevy::ecs::entity_disabling::Disabled>();
        app.update();
        assert!(app.world().get::<SceneViewLayers>(camera).is_some());
        app.world_mut()
            .entity_mut(replacement)
            .insert(CameraRetiring);
        app.update();
        assert!(app.world().get::<SceneViewLayers>(camera).is_none());
    }

    #[test]
    fn only_the_visible_explicit_main_binding_publishes_main_demand() {
        let mut app = App::new();
        app.init_resource::<lunco_viewport_core::SceneViewport>()
            .add_systems(
                Update,
                (publish_main_view_demand, publish_scene_view_layers).chain(),
            );
        let source = app.world_mut().spawn_empty().id();
        let first = output(app.world_mut(), source, 1);
        let second = output(app.world_mut(), source, 2);
        for camera in [first, second] {
            app.world_mut()
                .entity_mut(camera)
                .remove::<SceneViewDemand>()
                .insert(SceneCamera::default());
        }
        app.world_mut()
            .resource_mut::<lunco_viewport_core::SceneViewport>()
            .active_camera = Some(first);
        app.update();
        assert_eq!(
            app.world().get::<SceneViewDemand>(first).unwrap().output,
            SceneViewOutput::MainViewport
        );
        assert!(app.world().get::<SceneViewDemand>(second).is_none());
        app.world_mut()
            .resource_mut::<lunco_viewport_core::SceneViewport>()
            .active_camera = Some(second);
        app.update();
        assert!(app.world().get::<SceneViewLayers>(first).is_none());
        assert_eq!(
            app.world().get::<SceneViewDemand>(second).unwrap().source,
            second
        );
        app.world_mut()
            .resource_mut::<lunco_viewport_core::SceneViewport>()
            .visible = false;
        app.update();
        assert!(app.world().get::<SceneViewDemand>(second).is_none());
        assert!(app.world().get::<SceneViewLayers>(second).is_none());
    }

    #[test]
    fn selected_main_shadow_origin_yields_to_an_explicit_origin_and_returns_after_removal() {
        let mut app = App::new();
        app.init_resource::<lunco_viewport_core::SceneViewport>()
            .add_systems(
                Update,
                (
                    publish_main_view_demand,
                    publish_scene_view_layers,
                    publish_default_shadow_origin,
                )
                    .chain(),
            );
        let source = app.world_mut().spawn_empty().id();
        let camera = output(app.world_mut(), source, 1);
        app.world_mut()
            .entity_mut(camera)
            .remove::<SceneViewDemand>()
            .insert(SceneCamera::default());
        app.world_mut()
            .resource_mut::<lunco_viewport_core::SceneViewport>()
            .active_camera = Some(camera);
        app.update();
        assert!(
            app.world()
                .get::<bevy::camera::ShadowLodOrigin>(camera)
                .is_some()
        );
        app.world_mut()
            .entity_mut(source)
            .insert(bevy::camera::ShadowLodOrigin);
        app.update();
        assert!(
            app.world()
                .get::<bevy::camera::ShadowLodOrigin>(camera)
                .is_none()
        );
        assert!(
            app.world()
                .get::<bevy::camera::ShadowLodOrigin>(source)
                .is_some()
        );
        app.world_mut()
            .entity_mut(source)
            .remove::<bevy::camera::ShadowLodOrigin>();
        app.update();
        assert!(
            app.world()
                .get::<bevy::camera::ShadowLodOrigin>(camera)
                .is_some()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_render::camera::BloomLook;

    fn valid_quality_profile() -> lunco_render::RenderQualityProfile {
        lunco_render::RenderQualityProfile {
            directional_shadow_map_size: 1024,
            point_shadow_map_size: 512,
            directional_cascades: 2,
            shadow_filtering_quality: lunco_render::ShadowFilteringQuality::Hardware2x2,
            max_directional_shadow_casters: 1,
            max_point_shadow_casters: 1,
            max_spot_shadow_casters: 1,
            shadow_budget_bytes: 1024 * 1024 * 1024,
            horizon_shadow_cache_enabled: false,
            horizon_shadow_cache_sun_threshold_deg: 0.2,
            horizon_march_steps: 24,
            horizon_cache_samples_per_axis: 1,
            shadow_minimum_distance: 0.1,
            shadow_first_cascade_far_bound: 20.0,
            shadow_maximum_distance: 600.0,
            shadow_cascade_overlap: 0.1,
            shadow_depth_bias: 0.1,
            shadow_normal_bias: 4.0,
            camera_tone_map: ToneMap::AgX,
            camera_msaa: MsaaLevel::Off,
            camera_exposure_ev100: 16.0,
            render_failure_quiet_period_secs: 0.5,
            render_failure_give_up_after_secs: 5.0,
            editor_camera_bloom_enabled: false,
            camera_bloom_intensity: 0.0,
            camera_bloom_low_frequency_boost: 0.7,
            editor_preview_illuminance: 128_000.0,
            local_light_default_range: 20.0,
            local_shadow_map_near_z: 0.2,
            dome_cubemap_face_size: 512,
            primitive_sphere_longitudes: 24,
            primitive_sphere_latitudes: 16,
            primitive_radial_segments: 32,
            primitive_capsule_longitudes: 16,
            primitive_capsule_latitudes: 8,
            terrain_mesh_cache_bytes: 256 * 1024 * 1024,
            terrain_derived_map_resolution: 512,
            terrain_derived_ao_directions: 4,
            terrain_derived_ao_steps: 4,
            terrain_derived_ao_radius_fraction: 0.1,
            terrain_derived_roughness_base: 0.6,
            terrain_derived_roughness_saturation_radians: 0.6,
            terrain_derived_texture_anisotropy: 1,
            terrain_rock_max_instances: 2_000,
            terrain_rock_mesh_buckets: 3,
            terrain_rock_mesh_cube_count: 2,
            terrain_rock_lod_start_distance: 1_500.0,
            terrain_rock_lod_fade_distance: 300.0,
            terrain_lod_tile_resolution: 33,
            terrain_lod_cinematic_resolution: 1025,
            terrain_lod_pixel_error: 4.0,
            terrain_lod_max_depth: 6,
            terrain_lod_probe_resolution: 5,
            terrain_lod_bakes_per_frame: 8,
            terrain_lod_max_inflight_bakes: 16,
            terrain_lod_tile_budget: 256,
            terrain_lod_cover_edits_per_frame: 16,
            terrain_lod_hysteresis_ratio: 1.2,
            terrain_lod_morph_start_ratio: 0.45,
            nurbs_surface_samples_per_control_span: 3,
            nurbs_surface_minimum_subdivisions: 6,
            nurbs_surface_maximum_subdivisions: 64,
            nurbs_trim_curve_samples: 12,
            nurbs_trim_minimum_subdivisions: 8,
            nurbs_trim_maximum_subdivisions: 48,
            curve_samples_per_segment: 4,
            curve_radial_segments: 6,
        }
    }

    fn app() -> App {
        let mut a = App::new();
        a.add_plugins(MinimalPlugins);
        let mut settings = lunco_render::RenderingQualitySettings::default();
        settings.apply_profile(valid_quality_profile());
        assert!(settings.validated_profile().is_ok());
        a.insert_resource(settings);
        build(&mut a);
        a
    }

    /// The camera identity is render-free: a domain crate filters on `SceneCamera`,
    /// and the pipeline half is attached here.
    #[test]
    fn scene_camera_gains_a_pipeline() {
        let mut a = app();
        let e = a.world_mut().spawn(SceneCamera::agx()).id();
        a.update();
        assert!(a.world().entity(e).contains::<Camera3d>());
        assert_eq!(
            a.world().entity(e).get::<Tonemapping>(),
            Some(&Tonemapping::AgX)
        );
    }

    #[test]
    fn scene_camera_clears_starfield_backdrop_to_black() {
        let mut a = app();
        let e = a.world_mut().spawn(SceneCamera::default()).id();
        a.update();

        let ClearColorConfig::Custom(clear_color) =
            a.world().entity(e).get::<Camera>().unwrap().clear_color
        else {
            panic!("scene cameras must use an explicit clear color");
        };
        assert_eq!(clear_color.to_linear(), Color::BLACK.to_linear());
    }

    #[test]
    fn scene_camera_disables_unused_cluster_grid() {
        let mut a = app();
        let e = a.world_mut().spawn(SceneCamera::default()).id();
        a.update();

        assert!(matches!(
            a.world().entity(e).get::<ClusterConfig>(),
            Some(ClusterConfig::Single)
        ));
        assert!(a.world().entity(e).contains::<AutoClusterConfig>());
    }

    #[test]
    fn direct_render_camera_uses_the_same_cluster_policy() {
        let mut a = app();
        let e = a.world_mut().spawn(Camera3d::default()).id();
        a.update();

        assert!(matches!(
            a.world().entity(e).get::<ClusterConfig>(),
            Some(ClusterConfig::Single)
        ));
        assert!(a.world().entity(e).contains::<AutoClusterConfig>());
    }

    #[test]
    fn automatic_cluster_config_tracks_light_lifecycle() {
        let mut a = app();
        let camera = a.world_mut().spawn(SceneCamera::default()).id();
        a.update();
        let point_light = a.world_mut().spawn(PointLight::default()).id();
        a.update();

        assert!(matches!(
            a.world().entity(camera).get::<ClusterConfig>(),
            Some(ClusterConfig::FixedZ { .. })
        ));

        a.world_mut().despawn(point_light);
        a.update();
        assert!(matches!(
            a.world().entity(camera).get::<ClusterConfig>(),
            Some(ClusterConfig::Single)
        ));
    }

    #[test]
    fn cluster_policy_covers_entities_spawned_before_first_update() {
        let mut a = app();
        a.world_mut().spawn(PointLight::default());
        let camera = a.world_mut().spawn(Camera3d::default()).id();
        a.update();

        assert!(matches!(
            a.world().entity(camera).get::<ClusterConfig>(),
            Some(ClusterConfig::FixedZ { .. })
        ));
    }

    #[test]
    fn explicit_cluster_config_is_not_rewritten() {
        let mut a = app();
        let camera = a
            .world_mut()
            .spawn((SceneCamera::default(), ClusterConfig::Single))
            .id();
        a.update();

        assert!(matches!(
            a.world().entity(camera).get::<ClusterConfig>(),
            Some(ClusterConfig::Single)
        ));
        assert!(!a.world().entity(camera).contains::<AutoClusterConfig>());
    }

    /// **R4, half one.** MSAA was never configured anywhere, so Bevy ran its
    /// default 4× on a full-screen terrain. The balanced request is 2× on every
    /// target unless Graphics settings explicitly choose another value.
    #[test]
    fn msaa_is_actually_configured() {
        let mut a = app();
        let e = a.world_mut().spawn(SceneCamera::default()).id();
        a.update();
        let expected = Msaa::Sample2;
        assert_eq!(a.world().entity(e).get::<Msaa>(), Some(&expected));
    }

    /// **R4, half two.** Four crates configured Bloom on a camera with no HDR target,
    /// where it renders nothing and still costs a downsample/upsample chain. Asking
    /// for it without hdr must be refused, not silently honoured.
    #[test]
    fn bloom_without_hdr_is_refused() {
        let mut a = app();
        let e = a
            .world_mut()
            .spawn((SceneCamera {
                bloom: Some(BloomLook::new(0.15, 0.7)),
                hdr: false,
                ..Default::default()
            },))
            .id();
        a.update();
        assert!(
            !a.world().entity(e).contains::<Bloom>(),
            "bloom on an LDR camera must not attach"
        );
    }

    /// ...and `with_bloom` makes the correct thing the easy thing.
    #[test]
    fn with_bloom_turns_on_hdr() {
        let mut a = app();
        let e = a
            .world_mut()
            .spawn(SceneCamera::default().with_bloom(BloomLook::new(0.15, 0.7)))
            .id();
        a.update();
        assert!(a.world().entity(e).contains::<Bloom>());
        assert!(a.world().entity(e).contains::<Hdr>(), "bloom implies hdr");
    }

    #[test]
    fn fast_profile_turns_off_hdr_bloom_and_msaa() {
        let mut a = app();
        a.insert_resource(RenderProfile::Fast);
        let e = a
            .world_mut()
            .spawn(SceneCamera::default().with_bloom(BloomLook::new(0.15, 0.7)))
            .id();
        a.update();

        let entity = a.world().entity(e);
        assert_eq!(entity.get::<Msaa>(), Some(&Msaa::Off));
        assert_eq!(entity.get::<Tonemapping>(), Some(&Tonemapping::Linear));
        assert!(!entity.contains::<Hdr>());
        assert!(!entity.contains::<Bloom>());
    }

    #[test]
    fn graphics_settings_update_only_engine_owned_cameras() {
        let mut a = app();
        let owned = a
            .world_mut()
            .spawn((SceneCamera::default(), lunco_render::GraphicsCameraDefaults))
            .id();
        let explicit = a
            .world_mut()
            .spawn(SceneCamera {
                bloom: Some(BloomLook::new(0.4, 0.2)),
                hdr: false,
                ..Default::default()
            })
            .id();
        a.update();

        a.world_mut()
            .resource_mut::<lunco_render::RenderingQualitySettings>()
            .camera_bloom_intensity = 0.0;
        a.update();

        assert!(
            a.world()
                .entity(owned)
                .get::<SceneCamera>()
                .unwrap()
                .bloom
                .is_none()
        );
        assert!(!a.world().entity(owned).get::<SceneCamera>().unwrap().hdr);
        assert_eq!(
            a.world()
                .entity(explicit)
                .get::<SceneCamera>()
                .unwrap()
                .bloom,
            Some(BloomLook::new(0.4, 0.2))
        );
        assert!(!a.world().entity(explicit).get::<SceneCamera>().unwrap().hdr);
    }

    #[test]
    fn editor_bloom_policy_survives_scene_override_and_graphics_changes() {
        let mut profile = lunco_render::RenderingQualitySettings::default().profile();
        profile.camera_bloom_low_frequency_boost = 0.7;
        let mut editor = SceneCamera::default();
        let mut scene = SceneCamera::default();
        apply_camera_quality(&mut editor, profile, Some(0.42), true);
        apply_camera_quality(&mut scene, profile, Some(0.42), false);
        assert!(editor.bloom.is_none());
        assert!(!editor.hdr);
        assert_eq!(scene.bloom, Some(BloomLook::new(0.42, 0.7)));
        assert!(scene.hdr);
        profile.editor_camera_bloom_enabled = true;
        apply_camera_quality(&mut editor, profile, Some(0.42), true);
        assert_eq!(editor.bloom, scene.bloom);
        profile.editor_camera_bloom_enabled = false;
        apply_camera_quality(&mut editor, profile, Some(0.42), true);
        assert!(editor.bloom.is_none());
        // Verify that the pipeline actually removes an existing bloom pass.
        let mut a = app();
        let e = a.world_mut().spawn(scene).id();
        a.update();
        assert!(a.world().entity(e).contains::<Bloom>());
        *a.world_mut()
            .entity_mut(e)
            .get_mut::<SceneCamera>()
            .unwrap() = editor;
        a.update();
        assert!(!a.world().entity(e).contains::<Bloom>());
    }

    #[test]
    fn authored_scene_bloom_overrides_graphics_default() {
        let mut a = app();
        let e = a
            .world_mut()
            .spawn((SceneCamera::default(), lunco_render::GraphicsCameraDefaults))
            .id();
        a.update();

        a.world_mut()
            .resource_mut::<lunco_render::SceneBloomOverride>()
            .intensity = Some(0.42);
        a.update();

        assert_eq!(
            a.world().entity(e).get::<SceneCamera>().unwrap().bloom,
            Some(BloomLook::new(0.42, 0.7))
        );
        assert!(a.world().entity(e).get::<SceneCamera>().unwrap().hdr);
    }

    fn despawn_scene_cameras(mut commands: Commands, cameras: Query<Entity, With<SceneCamera>>) {
        for entity in &cameras {
            commands.entity(entity).try_despawn();
        }
    }

    #[test]
    fn changed_camera_rebind_ignores_deferred_scene_despawn() {
        let mut a = app();
        let entity = a
            .world_mut()
            .spawn((SceneCamera::default(), Camera::default()))
            .id();
        a.update();

        a.world_mut()
            .get_mut::<SceneCamera>(entity)
            .unwrap()
            .tone_map = ToneMap::AgX;
        a.add_systems(
            Update,
            despawn_scene_cameras.before(rebind_changed_scene_camera),
        );
        a.update();

        assert!(a.world().get_entity(entity).is_err());
    }
}
