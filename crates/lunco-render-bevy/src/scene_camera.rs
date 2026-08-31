//! Binds [`SceneCamera`] intent to a real render pipeline.
//!
//! Domain crates spawn `Camera` + `SceneCamera` (both render-free) and filter on
//! `With<SceneCamera>`. This module attaches the `bevy_core_pipeline` half —
//! `Camera3d`, tonemapping, MSAA, bloom — which is what actually costs wgpu.
//!
//! See `lunco_render::camera` for why, and for the two `R4` bugs this closes.

use bevy::camera::{ClearColorConfig, Hdr};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::{cluster::ClusterConfig, ClusteredDecal, LightProbe, PointLight, SpotLight};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use lunco_render::camera::{MsaaLevel, SceneCamera, ToneMap};

use crate::RenderProfile;

pub(crate) fn build(app: &mut App) {
    let initial_clusterable_object_count = count_clusterable_objects(app.world_mut());
    app.init_resource::<RenderProfile>()
        .insert_resource(ClusterableObjectCount(initial_clusterable_object_count))
        .init_resource::<lunco_render::RenderingQualitySettings>()
        .init_resource::<lunco_render::SceneBloomOverride>()
        .add_observer(configure_added_scene_camera)
        .add_observer(bind_scene_camera)
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
        );
}

fn tonemapping_of(t: ToneMap) -> Tonemapping {
    match t {
        ToneMap::None => Tonemapping::None,
        ToneMap::TonyMcMapface => Tonemapping::TonyMcMapface,
        ToneMap::AgX => Tonemapping::AgX,
        ToneMap::AcesFitted => Tonemapping::AcesFitted,
        ToneMap::Reinhard => Tonemapping::Reinhard,
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
    _trigger: On<Add, T>,
    mut count: ResMut<ClusterableObjectCount>,
) {
    count.add();
}

fn track_clusterable_object_removed<T: Component>(
    _trigger: On<Remove, T>,
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
    add: On<Add, Camera3d>,
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
        (Tonemapping::None, Msaa::Off)
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
    mut cameras: Query<&mut SceneCamera, With<lunco_render::GraphicsCameraDefaults>>,
) {
    let profile = match settings.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            warn!("invalid Graphics camera settings: {reason}; preserving current camera intent");
            return;
        }
    };
    for mut camera in &mut cameras {
        apply_camera_quality(&mut camera, profile, bloom_override.intensity);
    }
}

fn sync_new_scene_camera(
    settings: Res<lunco_render::RenderingQualitySettings>,
    bloom_override: Res<lunco_render::SceneBloomOverride>,
    mut cameras: Query<
        &mut SceneCamera,
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
    for mut camera in &mut cameras {
        apply_camera_quality(&mut camera, profile, bloom_override.intensity);
    }
}

fn apply_camera_quality(
    camera: &mut SceneCamera,
    profile: lunco_render::RenderQualityProfile,
    bloom_override: Option<f32>,
) {
    camera.tone_map = profile.camera_tone_map;
    camera.msaa = profile.camera_msaa;
    let bloom_intensity = bloom_override.unwrap_or(profile.camera_bloom_intensity);
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
    add: On<Add, Camera>,
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
    add: On<Add, SceneCamera>,
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
mod tests {
    use super::*;
    use lunco_render::camera::BloomLook;

    fn app() -> App {
        let mut a = App::new();
        a.add_plugins(MinimalPlugins);
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
        assert_eq!(entity.get::<Tonemapping>(), Some(&Tonemapping::None));
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

        assert!(a
            .world()
            .entity(owned)
            .get::<SceneCamera>()
            .unwrap()
            .bloom
            .is_none());
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
