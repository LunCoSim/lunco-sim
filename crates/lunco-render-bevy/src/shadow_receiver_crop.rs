//! Bound directional raster work to the native cascade's possible receivers.
//!
//! This is render culling, not a different shadow projection: all casters, depth
//! tests, cascade matrices, texels and filtering remain native. Unknown receiver
//! domains keep the full native raster. Source admission is asset scoped; a
//! modified shader cannot be readmitted while an older pipeline
//! for that asset can still exist.

use crate::{ShaderMaterial, ShaderSchemas};
use bevy::{
    asset::AssetId,
    camera::visibility::{RenderLayers, VisibilitySystems},
    light::{
        CascadeShadowConfig, Cascades, DirectionalLightShadowMap, ShadowFilteringMethod,
        SimulationLightSystems, cascade::Cascade,
    },
    math::DVec2,
    pbr::{
        ExtractedDirectionalLight, LightEntity, MeshMaterial3d, Shadow, StandardMaterial,
        ViewLightEntities, prepare_lights,
    },
    platform::collections::HashMap,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        camera::TemporalJitter,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_phase::{Draw, DrawError, DrawFunctions, TrackedRenderPass},
        sync_world::RenderEntity,
        view::ExtractedView,
    },
    shader::Shader,
};
use lunco_core::{AppSceneExt, SceneResource};

/// Every directional shadow read stays at the rasterized surface, uses its
/// view-space depth, and supplies a normal of length at most one (or reads no shadows).
const SURFACE_RECEIVER: &str = "lunco.shadow-receiver.surface.v1";

#[derive(Resource, Default, Clone, ExtractResource)]
#[extract_app(RenderApp)]
struct ReceiverCropInputs(HashMap<(Entity, Entity, usize), (UVec4, u32)>);
impl SceneResource for ReceiverCropInputs {}

#[derive(Resource, Default)]
struct ReceiverCropViews(HashMap<Entity, (UVec4, u32)>);

/// Main-world render admission and projected work; GPU acceptance is measured
/// independently. This component belongs to the camera and retires with it.
#[derive(Component, Default, Clone, PartialEq, Reflect)]
#[reflect(Component)]
struct DirectionalShadowRaster {
    cropped_cascades: u32,
    rasterized_texels: u64,
    full_texels: u64,
    reason: String,
}

#[derive(Resource, Default)]
struct ReceiverMaterials {
    standard: HashMap<AssetId<StandardMaterial>, bool>,
    custom: HashMap<AssetId<ShaderMaterial>, bool>,
    // false is irreversible until the Shader asset is removed. An unbounded
    // compiled pipeline can outlive a source reload or a material replacement.
    source_contracts: HashMap<AssetId<Shader>, bool>,
}

#[derive(Resource, Default)]
struct WrappedShadowDraws(usize);

pub(super) fn build(app: &mut App) {
    app.register_type::<DirectionalShadowRaster>()
        .register_type::<TemporalJitter>()
        .init_resource::<ReceiverMaterials>()
        .init_scene_resource::<ReceiverCropInputs>()
        .add_plugins(ExtractResourcePlugin::<ReceiverCropInputs>::default())
        .add_systems(
            PostUpdate,
            (
                admit_materials.after(bevy::asset::AssetEventSystems),
                project_receiver_bounds
                    .after(admit_materials)
                    .after(SimulationLightSystems::UpdateDirectionalLightCascades)
                    .after(VisibilitySystems::CheckVisibility),
            ),
        );
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<ReceiverCropViews>()
            .init_resource::<WrappedShadowDraws>()
            .add_systems(
                Render,
                (
                    wrap_shadow_draws.in_set(RenderSystems::Prepare),
                    prepare_receiver_views
                        .in_set(RenderSystems::PrepareViews)
                        .after(prepare_lights),
                ),
            );
    }
}

fn admit_materials(
    mut admission: ResMut<ReceiverMaterials>,
    standard: Res<Assets<StandardMaterial>>,
    custom: Res<Assets<ShaderMaterial>>,
    schemas: Res<ShaderSchemas>,
    shaders: Res<Assets<Shader>>,
    mut standard_events: MessageReader<AssetEvent<StandardMaterial>>,
    mut custom_events: MessageReader<AssetEvent<ShaderMaterial>>,
    mut shader_events: MessageReader<AssetEvent<Shader>>,
) {
    let standard_changed = standard_events.read().count() != 0;
    let custom_changed = custom_events.read().count() != 0;
    // Asset publication occurs after Update. Discard a proof immediately,
    // before extraction, even if reflection will run only next Update.
    let mut sources_changed = false;
    for event in shader_events.read() {
        match event {
            AssetEvent::Modified { id } => {
                // A changed module may be an imported or embedded library.
                // Retire current proofs without duplicating native import
                // resolution or racing its compiled pipeline dependency graph.
                for proof in admission.source_contracts.values_mut() {
                    *proof = false;
                }
                admission.source_contracts.insert(*id, false);
                sources_changed = true;
            }
            AssetEvent::Removed { id } => {
                admission.source_contracts.remove(id);
                sources_changed = true;
            }
            _ => {}
        }
    }
    if admission.is_added() || standard_changed {
        admission.standard.clear();
        for (id, material) in standard.iter() {
            admission
                .standard
                .insert(id, material.unlit || material.diffuse_transmission == 0.0);
        }
    }
    if admission.is_added() || custom_changed || schemas.is_changed() || sources_changed {
        admission.custom.clear();
        for (id, material) in custom.iter() {
            let bounded = shaders.contains(material.shader.id())
                && schemas.get(material.shader.id()).is_some_and(|schema| {
                    std::sync::Arc::ptr_eq(schema, &material.schema)
                        && schema.capabilities.contains(SURFACE_RECEIVER)
                });
            // A pending initial source has no compiled consumer; do not latch
            // its absence. A reflected unbounded source must remain unbounded.
            if shaders.contains(material.shader.id()) && schemas.get(material.shader.id()).is_some()
            {
                let proof = admission
                    .source_contracts
                    .entry(material.shader.id())
                    .or_insert(bounded);
                *proof &= bounded;
            }
            let admitted =
                bounded && admission.source_contracts.get(&material.shader.id()) == Some(&true);
            admission.custom.insert(id, admitted);
        }
    }
}

fn project_receiver_bounds(
    mut commands: Commands,
    mut output: ResMut<ReceiverCropInputs>,
    admission: Res<ReceiverMaterials>,
    size: Res<DirectionalLightShadowMap>,
    lights: Query<(
        &RenderEntity,
        &DirectionalLight,
        &CascadeShadowConfig,
        &Cascades,
    )>,
    mut cameras: Query<
        (
            Entity,
            &RenderEntity,
            &Camera,
            &Projection,
            &GlobalTransform,
            Option<&ShadowFilteringMethod>,
            Option<&RenderLayers>,
            Has<TemporalJitter>,
            Option<&mut DirectionalShadowRaster>,
        ),
        With<Camera3d>,
    >,
    receivers: Query<
        (
            &InheritedVisibility,
            Option<&RenderLayers>,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&MeshMaterial3d<ShaderMaterial>>,
        ),
        With<Mesh3d>,
    >,
) {
    output.0.clear();
    let size = size.size as u32;
    for (
        camera_id,
        camera_entity,
        camera,
        projection,
        transform,
        filtering,
        layers,
        jitter,
        previous,
    ) in &mut cameras
    {
        let mut status = DirectionalShadowRaster::default();
        let radius = match filtering.copied().unwrap_or_default() {
            ShadowFilteringMethod::Hardware2x2 => Some(1.0),
            ShadowFilteringMethod::Gaussian => Some(3.0),
            ShadowFilteringMethod::Temporal => None,
        };
        let near = match projection {
            // Include the apex, even when the cascade configuration starts
            // farther away than the actual camera near plane.
            Projection::Perspective(p)
                if p.near_clip_plane.x == 0.0 && p.near_clip_plane.y == 0.0 =>
            {
                Some(0.0)
            }
            Projection::Orthographic(p) => Some(p.near),
            _ => None,
        };
        let domain_bounded = camera.is_active
            && !jitter
            && camera.sub_camera_view.is_none()
            && near.is_some()
            && radius.is_some()
            && receivers
                .iter()
                .all(|(visibility, receiver_layers, standard, custom)| {
                    if !visibility.get()
                        || !layers
                            .cloned()
                            .unwrap_or_default()
                            .intersects(&receiver_layers.cloned().unwrap_or_default())
                    {
                        return true;
                    }
                    match (standard, custom) {
                        (Some(m), None) => admission.standard.get(&m.id()) == Some(&true),
                        (None, Some(m)) => admission.custom.get(&m.id()) == Some(&true),
                        _ => false,
                    }
                });
        let reason = if !camera.is_active {
            "inactive-camera"
        } else if jitter {
            "temporal-jitter"
        } else if camera.sub_camera_view.is_some() || near.is_none() {
            "unbounded-projection"
        } else if radius.is_none() {
            "unbounded-filter"
        } else if !domain_bounded {
            "unbounded-material"
        } else {
            "receiver-bounded"
        };
        status.reason = reason.to_owned();
        for (light_entity, light, config, cascades) in &lights {
            let Some(slices) = cascades.cascades.get(&camera_id) else {
                continue;
            };
            if !light.shadow_maps_enabled {
                continue;
            }
            let near_bounds = std::iter::once(near.unwrap_or(0.0)).chain(
                config
                    .bounds
                    .iter()
                    .map(|far| (1.0 - config.overlap_proportion) * far),
            );
            for (index, ((near, far), cascade)) in
                near_bounds.zip(&config.bounds).zip(slices).enumerate()
            {
                let full_texels = u64::from(size) * u64::from(size);
                status.full_texels += full_texels;
                let rect = if reason == "receiver-bounded" {
                    receiver_rect(
                        projection,
                        transform.to_matrix(),
                        near,
                        *far,
                        cascade,
                        light,
                        size,
                        radius.unwrap(),
                    )
                } else {
                    None
                };
                if let Some(rect) = rect {
                    status.cropped_cascades += 1;
                    status.rasterized_texels += u64::from(rect.z) * u64::from(rect.w);
                    output
                        .0
                        .insert((light_entity.id(), camera_entity.id(), index), (rect, size));
                } else {
                    status.rasterized_texels += full_texels;
                }
            }
        }
        if let Some(mut previous) = previous {
            if *previous != status {
                *previous = status;
            }
        } else {
            commands.entity(camera_id).insert(status);
        }
    }
}

/// Use native frustum corners and the exact native cascade. Double arithmetic
/// computes bounds at the GPU boundary; an outward error envelope covers f32
/// corner construction and both matrix products, including large translations.
fn receiver_rect(
    projection: &Projection,
    world_from_camera: Mat4,
    near: f32,
    far: f32,
    cascade: &Cascade,
    light: &DirectionalLight,
    size: u32,
    filter_radius: f32,
) -> Option<UVec4> {
    let matrix = cascade.clip_from_world;
    if size == 0
        || !near.is_finite()
        || !far.is_finite()
        || near >= far
        || matrix.row(3) != Vec4::W
        || !matrix.is_finite()
        || !world_from_camera.is_finite()
        || !cascade.texel_size.is_finite()
        || cascade.texel_size <= 0.0
        || !light.shadow_depth_bias.is_finite()
        || !light.shadow_normal_bias.is_finite()
    {
        return None;
    }
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    let world_matrix = world_from_camera.as_dmat4();
    let clip_matrix = matrix.as_dmat4();
    for corner in projection.get_frustum_corners(-near, -far) {
        let point = corner.as_dvec3().extend(1.0);
        let world = world_matrix * point;
        let clip = clip_matrix * world;
        // Absolute products bound cancellation error, unlike a relative
        // epsilon applied only to the final, possibly near-zero coordinate.
        let world_magnitude = world_matrix.abs() * point.abs();
        let clip_magnitude = clip_matrix.abs() * world_magnitude;
        let error = clip_magnitude.truncate().truncate() * (64.0 * f32::EPSILON as f64);
        let uv = clip.truncate().truncate() * DVec2::new(0.5, -0.5) + DVec2::splat(0.5);
        lo = lo.min(uv - error);
        hi = hi.max(uv + error);
    }
    let bias_world = f64::from(light.shadow_normal_bias.abs()) * f64::from(cascade.texel_size)
        + f64::from(light.shadow_depth_bias.abs());
    let scale = DVec2::new(
        matrix.row(0).truncate().as_dvec3().length(),
        matrix.row(1).truncate().as_dvec3().length(),
    );
    let padding =
        scale * (bias_world * 0.5 * f64::from(size)) + DVec2::splat(f64::from(filter_radius) + 1.0);
    let lo = (lo * f64::from(size) - padding)
        .floor()
        .clamp(DVec2::ZERO, DVec2::splat(f64::from(size)));
    let hi = (hi * f64::from(size) + padding)
        .ceil()
        .clamp(DVec2::ZERO, DVec2::splat(f64::from(size)));
    if !lo.is_finite() || !hi.is_finite() || !hi.cmpgt(lo).all() {
        return None;
    }
    let lo = lo.as_uvec2();
    let extent = hi.as_uvec2() - lo;
    if extent.x == size && extent.y == size {
        return None;
    }
    Some(UVec4::new(lo.x, lo.y, extent.x, extent.y))
}

fn prepare_receiver_views(
    inputs: Res<ReceiverCropInputs>,
    mut output: ResMut<ReceiverCropViews>,
    cameras: Query<(Entity, &ViewLightEntities)>,
    lights: Query<&LightEntity>,
    directional: Query<&ExtractedDirectionalLight>,
) {
    output.0.clear();
    for (camera, views) in &cameras {
        for view in &views.lights {
            if let Ok(LightEntity::Directional {
                light_entity,
                cascade_index,
            }) = lights.get(*view)
                && directional
                    .get(*light_entity)
                    .is_ok_and(|light| light.soft_shadow_size.is_none_or(|size| size == 0.0))
                && let Some(rect) = inputs.0.get(&(*light_entity, camera, *cascade_index))
            {
                output.0.insert(*view, *rect);
            }
        }
    }
}

struct ReceiverCroppedDraw {
    inner: Box<dyn Draw<Shadow>>,
    applied_view: Option<Entity>,
}
impl Draw<Shadow> for ReceiverCroppedDraw {
    fn prepare(&mut self, world: &World) {
        // Native BinnedRenderPhase::render prepares every delegate before
        // encoding this pass. Its draws share one immutable view and raster.
        self.applied_view = None;
        self.inner.prepare(world);
    }
    fn draw<'w>(
        &mut self,
        world: &'w World,
        pass: &mut TrackedRenderPass<'w>,
        view: Entity,
        item: &Shadow,
    ) -> Result<(), DrawError> {
        if self.applied_view != Some(view) {
            let extracted = world
                .get::<ExtractedView>(view)
                .ok_or(DrawError::InvalidViewQuery)?;
            let full = UVec4::new(0, 0, extracted.viewport.z, extracted.viewport.w);
            let rect = world
                .resource::<ReceiverCropViews>()
                .0
                .get(&view)
                .filter(|(_, size)| *size == full.z && *size == full.w)
                .map(|(rect, _)| *rect)
                .unwrap_or(full);
            pass.set_scissor_rect(rect.x, rect.y, rect.z, rect.w);
            self.applied_view = Some(view);
        }
        self.inner.draw(world, pass, view, item)
    }
}

/// Draw registration is append-only. Keep native IDs and type indices intact;
/// wrap each delegate once, including commands added after GPU recovery.
fn wrap_shadow_draws(
    functions: Res<DrawFunctions<Shadow>>,
    mut wrapped: ResMut<WrappedShadowDraws>,
) {
    let mut functions = functions.write();
    if wrapped.0 == functions.draw_functions.len() {
        return;
    }
    let mut tail = functions.draw_functions.split_off(wrapped.0);
    functions.draw_functions.extend(tail.drain(..).map(|inner| {
        Box::new(ReceiverCroppedDraw {
            inner,
            applied_view: None,
        }) as Box<dyn Draw<Shadow>>
    }));
    wrapped.0 = functions.draw_functions.len();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_contains_native_bias_and_filter_support() {
        let projection = Projection::Perspective(PerspectiveProjection {
            fov: 0.7,
            aspect_ratio: 1.7,
            ..default()
        });
        let size = 8192;
        let light = DirectionalLight {
            shadow_normal_bias: 3.0,
            shadow_depth_bias: 0.02,
            ..default()
        };
        let mut cropped = 0;
        for translation in [
            Vec3::ZERO,
            Vec3::new(1000.0, -100.0, 900.0),
            Vec3::splat(1.0e6),
        ] {
            for rotation in [
                Quat::IDENTITY,
                Quat::from_euler(EulerRot::YXZ, 1.1, -0.4, 0.2),
            ] {
                let world = Mat4::from_rotation_translation(rotation, translation);
                let cascade = Cascade {
                    clip_from_world: bevy::math::proj::orthographic(
                        -50.0, 50.0, -50.0, 50.0, -100.0, 100.0,
                    ) * Mat4::from_translation(-translation),
                    texel_size: 100.0 / size as f32,
                    ..default()
                };
                for radius in [1.0, 3.0] {
                    let rect = receiver_rect(
                        &projection,
                        world,
                        0.0,
                        20.0,
                        &cascade,
                        &light,
                        size,
                        radius,
                    );
                    let rect = rect.unwrap_or(UVec4::new(0, 0, size, size));
                    cropped += usize::from(rect.z < size || rect.w < size);
                    for corner in projection.get_frustum_corners(0.0, -20.0) {
                        for normal in [
                            Vec3::X,
                            Vec3::NEG_X,
                            Vec3::Y,
                            Vec3::NEG_Y,
                            Vec3::Z,
                            Vec3::NEG_Z,
                        ] {
                            let position: Vec3 = world.transform_point3a(corner).into();
                            let biased = position
                                + normal * light.shadow_normal_bias * cascade.texel_size
                                + Vec3::Z * light.shadow_depth_bias;
                            let clip = cascade.clip_from_world * biased.extend(1.0);
                            let pixel = (clip.truncate().truncate() * Vec2::new(0.5, -0.5)
                                + Vec2::splat(0.5))
                                * size as f32;
                            // Coordinates outside the native map already return
                            // lit; only the clamped texture domain is relevant.
                            let lower = (pixel - Vec2::splat(radius))
                                .clamp(Vec2::ZERO, Vec2::splat(size as f32));
                            let upper = (pixel + Vec2::splat(radius))
                                .clamp(Vec2::ZERO, Vec2::splat(size as f32));
                            assert!(lower.cmpge(rect.xy().as_vec2()).all(), "{rect:?} {pixel:?}");
                            assert!(
                                upper.cmple((rect.xy() + rect.zw()).as_vec2()).all(),
                                "{rect:?} {pixel:?}"
                            );
                        }
                    }
                }
            }
        }
        assert!(cropped > 0, "the proof must admit useful rectangles");
        let mut cascade = Cascade {
            clip_from_world: Mat4::IDENTITY,
            texel_size: 1.0,
            ..default()
        };
        assert!(
            receiver_rect(
                &projection,
                Mat4::IDENTITY,
                1.0,
                0.0,
                &cascade,
                &light,
                size,
                1.0
            )
            .is_none()
        );
        cascade.clip_from_world.x_axis.x = f32::NAN;
        assert!(
            receiver_rect(
                &projection,
                Mat4::IDENTITY,
                0.0,
                20.0,
                &cascade,
                &light,
                size,
                1.0
            )
            .is_none()
        );
    }
}
