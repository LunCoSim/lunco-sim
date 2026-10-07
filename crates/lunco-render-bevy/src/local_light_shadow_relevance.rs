//! Avoid preparing local-light shadow views that cannot contribute to any output.
//!
//! Bevy shares local-light shadow maps across cameras. Its normal path keeps
//! maps for visible lights even when their bounded influence misses every active
//! 3D camera. This render-world adapter checks a conservative point-light range
//! sphere or spotlight-cone bound against every extracted 3D camera and
//! suppresses only that light's shadow-map flag. The main-world light, direct
//! illumination, influence range, and shadow quality settings remain unchanged.

use bevy::{
    camera::{
        Camera3d,
        primitives::{Frustum, Sphere},
        visibility::RenderLayers,
    },
    pbr::{ExtractedPointLight, PointAndSpotLightViewEntities},
    prelude::{
        AnyOf, App, Changed, Commands, Component, Entity, IntoScheduleConfigs, On, Or, PointLight,
        Query, Remove, SpotLight, With, World,
    },
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems, camera::ExtractedCamera,
        sync_world::RenderEntity,
    },
};

/// The main-world light's shadow intent, independent of render-frame culling.
#[derive(Component)]
struct LocalLightShadowIntent(bool);

pub(super) fn build(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };

    render_app.add_systems(ExtractSchedule, extract_local_light_shadow_intent);
    render_app
        .world_mut()
        .add_observer(retire_removed_local_light_shadow_views);
    render_app.add_systems(
        Render,
        suppress_irrelevant_local_light_shadows
            .in_set(RenderSystems::CreateViews)
            .before(bevy::pbr::prepare_lights),
    );
}

fn retire_removed_local_light_shadow_views(
    event: On<Remove, ExtractedPointLight>,
    mut commands: Commands,
) {
    let entity = event.entity;
    commands.queue(move |world: &mut World| retire_absent_local_light_shadow_views(world, entity));
}

fn retire_absent_local_light_shadow_views(world: &mut World, entity: Entity) {
    let Ok(mut light) = world.get_entity_mut(entity) else {
        return;
    };
    if !light.contains::<ExtractedPointLight>() {
        // Removing the native tracker invokes Bevy's own shadow-view cleanup.
        // Re-extraction installs a fresh required tracker before prepare_lights.
        light.remove::<PointAndSpotLightViewEntities>();
    }
}

fn extract_local_light_shadow_intent(
    mut commands: Commands,
    lights: Extract<
        Query<
            (&RenderEntity, AnyOf<(&PointLight, &SpotLight)>),
            Or<(
                Changed<RenderEntity>,
                Changed<PointLight>,
                Changed<SpotLight>,
            )>,
        >,
    >,
) {
    for (entity, (point, spot)) in &lights {
        let enabled = point
            .map(|light| light.shadow_maps_enabled)
            .or_else(|| spot.map(|light| light.shadow_maps_enabled));
        if let Some(enabled) = enabled {
            commands
                .entity(entity.id())
                .try_insert(LocalLightShadowIntent(enabled));
        }
    }
}

fn suppress_irrelevant_local_light_shadows(
    mut lights: Query<(
        &mut ExtractedPointLight,
        &LocalLightShadowIntent,
        &RenderLayers,
        Option<&Frustum>,
    )>,
    cameras: Query<(&Frustum, Option<&RenderLayers>), (With<Camera3d>, With<ExtractedCamera>)>,
) {
    let default_layers = RenderLayers::default();
    for (mut light, intent, light_layers, light_frustum) in &mut lights {
        // Native light extraction is incremental; always restore the source
        // intent before evaluating this frame's active output views.
        let shadow_maps_enabled = if !intent.0 || cameras.is_empty() {
            intent.0
        } else if cameras
            .iter()
            .all(|(_, layers)| !light_layers.intersects(layers.unwrap_or(&default_layers)))
        {
            // Layer separation is exact and does not require a finite cone bound.
            false
        } else {
            let light_bounds = if light.spot_light_angles.is_some() {
                light_frustum.and_then(conservative_frustum_sphere)
            } else {
                Some(point_light_influence_sphere(
                    light.transform.translation(),
                    light.range,
                ))
            };
            light_bounds.is_none_or(|bounds| {
                !local_light_shadow_is_irrelevant(
                    bounds,
                    light_layers,
                    cameras
                        .iter()
                        .map(|(frustum, layers)| (frustum, layers.unwrap_or(&default_layers))),
                )
            })
        };

        if light.shadow_maps_enabled != shadow_maps_enabled {
            light.shadow_maps_enabled = shadow_maps_enabled;
        }
    }
}

/// A sphere enclosing a point light's finite influence range.
fn point_light_influence_sphere(center: bevy::math::Vec3, range: f32) -> Sphere {
    Sphere {
        center: center.into(),
        radius: range,
    }
}

/// A sphere enclosing all corners of a finite spotlight frustum.
///
/// The sphere is only a broad-phase bound. Its larger-than-cone shape can keep
/// an unnecessary shadow map, but cannot reject a cone that reaches a camera.
fn conservative_frustum_sphere(frustum: &Frustum) -> Option<Sphere> {
    let corners = frustum.0.corners()?;
    if corners.iter().any(|corner| !corner.is_finite()) {
        return None;
    }

    let mut min = corners[0];
    let mut max = corners[0];
    for corner in corners.iter().skip(1) {
        min = min.min(*corner);
        max = max.max(*corner);
    }

    let center = min + (max - min) * 0.5;
    if !center.is_finite() {
        return None;
    }

    let radius = corners
        .iter()
        .map(|corner| center.distance(*corner))
        .fold(0.0_f32, f32::max);
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }

    Some(Sphere {
        center: center.into(),
        radius,
    })
}

fn local_light_shadow_is_irrelevant<'a>(
    light_bounds: Sphere,
    light_layers: &RenderLayers,
    camera_views: impl IntoIterator<Item = (&'a Frustum, &'a RenderLayers)>,
) -> bool {
    camera_views.into_iter().all(|(frustum, camera_layers)| {
        !light_layers.intersects(camera_layers)
            || !frustum_intersects_sphere_conservatively(frustum, light_bounds)
    })
}

/// Intersect using Bevy's frustum test with a small floating-point error bound.
///
/// The pad scales with the plane dot-product terms, so it remains conservative
/// when a view is translated far from its local origin. Invalid plane data is
/// treated as intersecting; uncertainty must retain the authored shadow map.
fn frustum_intersects_sphere_conservatively(frustum: &Frustum, sphere: Sphere) -> bool {
    if !sphere.center.is_finite() || !sphere.radius.is_finite() || sphere.radius <= 0.0 {
        return true;
    }

    let mut rounding_pad = 0.0_f32;
    for half_space in &frustum.half_spaces {
        let plane = half_space.normal_d();
        if plane.x == 0.0 && plane.y == 0.0 && plane.z == 0.0 && plane.w == f32::INFINITY {
            // Bevy represents an infinite far plane this way.
            continue;
        }
        if !plane.is_finite() {
            return true;
        }

        let dot_scale = plane.x.abs() * sphere.center.x.abs()
            + plane.y.abs() * sphere.center.y.abs()
            + plane.z.abs() * sphere.center.z.abs()
            + plane.w.abs()
            + sphere.radius;
        if !dot_scale.is_finite() {
            return true;
        }

        // Bounds accumulated rounding in the plane dot product and radius sum.
        rounding_pad = rounding_pad.max(dot_scale * f32::EPSILON * 8.0);
    }

    let padded_radius = sphere.radius + rounding_pad;
    if !padded_radius.is_finite() {
        return true;
    }

    frustum.intersects_sphere(
        &Sphere {
            center: sphere.center,
            radius: padded_radius,
        },
        true,
    )
}

#[cfg(test)]
mod tests {
    use bevy::{
        camera::Camera3d,
        camera::{
            CameraProjection, PerspectiveProjection,
            primitives::{Frustum, Sphere},
            visibility::RenderLayers,
        },
        ecs::system::RunSystemOnce,
        math::{
            Vec3,
            primitives::{HalfSpace, ViewFrustum},
        },
        prelude::{GlobalTransform, Transform},
        render::{Render, camera::ExtractedCamera},
    };

    use super::{
        conservative_frustum_sphere, frustum_intersects_sphere_conservatively,
        local_light_shadow_is_irrelevant, point_light_influence_sphere,
    };

    fn perspective_frustum(position: Vec3, far: f32, fov: f32) -> Frustum {
        PerspectiveProjection {
            far,
            fov,
            ..Default::default()
        }
        .compute_frustum(&GlobalTransform::from(Transform::from_translation(
            position,
        )))
    }

    fn axis_aligned_frustum() -> Frustum {
        Frustum(ViewFrustum {
            half_spaces: [
                HalfSpace::new(Vec3::X.extend(0.0)),
                HalfSpace::new(Vec3::NEG_X.extend(10.0)),
                HalfSpace::new(Vec3::Y.extend(10.0)),
                HalfSpace::new(Vec3::NEG_Y.extend(10.0)),
                HalfSpace::new(Vec3::Z.extend(100.0)),
                HalfSpace::new(Vec3::NEG_Z.extend(0.0)),
            ],
        })
    }

    #[test]
    fn spotlight_bounds_are_finite_and_enclose_the_frustum() {
        let spot_frustum = perspective_frustum(Vec3::ZERO, 90.0, 20.0_f32.to_radians());
        let bounds = conservative_frustum_sphere(&spot_frustum).unwrap();

        assert!(bounds.radius.is_finite());
        assert!(bounds.radius < 90.0);
        for corner in spot_frustum.0.corners().unwrap() {
            assert!(bounds.center.distance(corner.into()) <= bounds.radius);
        }
    }

    #[test]
    fn any_matching_camera_view_keeps_the_local_light_shadow() {
        let spotlight = conservative_frustum_sphere(&perspective_frustum(
            Vec3::new(100.0, 0.0, 0.0),
            20.0,
            20.0_f32.to_radians(),
        ))
        .unwrap();
        let light_layers = RenderLayers::default();
        let offscreen_view = perspective_frustum(Vec3::ZERO, 30.0, 60.0_f32.to_radians());
        let receiving_view =
            perspective_frustum(Vec3::new(100.0, 0.0, 0.0), 30.0, 60.0_f32.to_radians());

        assert!(!local_light_shadow_is_irrelevant(
            spotlight,
            &light_layers,
            [
                (&offscreen_view, &light_layers),
                (&receiving_view, &light_layers)
            ],
        ));
    }

    #[test]
    fn point_light_range_disjoint_from_the_camera_skips_its_shadow() {
        let light_bounds = point_light_influence_sphere(Vec3::new(100.0, 0.0, 0.0), 20.0);
        let light_layers = RenderLayers::default();
        let camera_frustum = perspective_frustum(Vec3::ZERO, 30.0, 60.0_f32.to_radians());

        assert!(local_light_shadow_is_irrelevant(
            light_bounds,
            &light_layers,
            [(&camera_frustum, &light_layers)],
        ));
    }

    #[test]
    fn point_light_influence_intersecting_the_camera_keeps_its_shadow() {
        let light_bounds = point_light_influence_sphere(Vec3::new(0.0, 0.0, -20.0), 100.0);
        let light_layers = RenderLayers::default();
        let camera_frustum = perspective_frustum(Vec3::ZERO, 30.0, 60.0_f32.to_radians());

        assert!(!local_light_shadow_is_irrelevant(
            light_bounds,
            &light_layers,
            [(&camera_frustum, &light_layers)],
        ));
    }

    fn extracted_camera() -> ExtractedCamera {
        use bevy::ecs::schedule::ScheduleLabel;

        ExtractedCamera {
            target: None,
            physical_viewport_size: None,
            physical_target_size: None,
            viewport: None,
            schedule: Render.intern(),
            order: 0,
            output_mode: Default::default(),
            msaa_writeback: Default::default(),
            clear_color: Default::default(),
            sorted_camera_index_for_target: 0,
            exposure: 1.0,
            hdr: false,
            compositing_space: None,
        }
    }

    fn extracted_point_light(position: Vec3, range: f32) -> bevy::pbr::ExtractedPointLight {
        bevy::pbr::ExtractedPointLight {
            color: bevy::color::LinearRgba::WHITE,
            intensity: 1.0,
            range,
            radius: 0.0,
            transform: GlobalTransform::from(Transform::from_translation(position)),
            shadow_maps_enabled: true,
            contact_shadows_enabled: false,
            shadow_depth_bias: 0.0,
            shadow_normal_bias: 0.0,
            shadow_map_near_z: 0.1,
            spot_light_angles: None,
            volumetric: false,
            soft_shadows_enabled: false,
            affects_lightmapped_mesh_diffuse: false,
        }
    }

    #[test]
    fn render_filter_skips_only_an_offscreen_point_light_shadow_map() {
        let mut world = bevy::prelude::World::new();
        let camera_frustum = perspective_frustum(Vec3::ZERO, 30.0, 60.0_f32.to_radians());
        world.spawn((
            Camera3d::default(),
            extracted_camera(),
            camera_frustum.clone(),
            RenderLayers::default(),
        ));
        let offscreen_light = world
            .spawn((
                extracted_point_light(Vec3::new(100.0, 0.0, 0.0), 20.0),
                super::LocalLightShadowIntent(true),
                RenderLayers::default(),
            ))
            .id();
        let visible_light = world
            .spawn((
                extracted_point_light(Vec3::new(0.0, 0.0, -20.0), 100.0),
                super::LocalLightShadowIntent(true),
                RenderLayers::default(),
            ))
            .id();

        world
            .run_system_once(super::suppress_irrelevant_local_light_shadows)
            .expect("render-world relevance filter must run");

        assert!(
            !world
                .get::<bevy::pbr::ExtractedPointLight>(offscreen_light)
                .unwrap()
                .shadow_maps_enabled
        );
        assert!(
            world
                .get::<bevy::pbr::ExtractedPointLight>(visible_light)
                .unwrap()
                .shadow_maps_enabled
        );
    }

    #[test]
    fn render_filter_restores_incremental_light_intent_and_parks_unbounded_spots() {
        let mut world = bevy::prelude::World::new();
        let camera = world
            .spawn((
                Camera3d::default(),
                extracted_camera(),
                perspective_frustum(Vec3::ZERO, 30.0, 60.0_f32.to_radians()),
                RenderLayers::layer(1),
            ))
            .id();
        let mut spot = extracted_point_light(Vec3::ZERO, 20.0);
        spot.spot_light_angles = Some((0.1, 0.2));
        let light = world
            .spawn((
                spot,
                super::LocalLightShadowIntent(true),
                RenderLayers::layer(2),
            ))
            .id();
        let disabled = world
            .spawn((
                extracted_point_light(Vec3::ZERO, 20.0),
                super::LocalLightShadowIntent(false),
                RenderLayers::layer(1),
            ))
            .id();
        let mut schedule = bevy::prelude::Schedule::default();
        schedule.add_systems(super::suppress_irrelevant_local_light_shadows);
        schedule.run(&mut world);
        assert!(
            !world
                .get::<bevy::pbr::ExtractedPointLight>(light)
                .unwrap()
                .shadow_maps_enabled
        );
        assert!(
            !world
                .get::<bevy::pbr::ExtractedPointLight>(disabled)
                .unwrap()
                .shadow_maps_enabled
        );
        // No native light update occurs while a matching output becomes active.
        world.entity_mut(camera).insert(RenderLayers::layer(2));
        schedule.run(&mut world);
        assert!(
            world
                .get::<bevy::pbr::ExtractedPointLight>(light)
                .unwrap()
                .shadow_maps_enabled
        );
        assert!(
            !world
                .get::<bevy::pbr::ExtractedPointLight>(disabled)
                .unwrap()
                .shadow_maps_enabled
        );
        world
            .get_mut::<super::LocalLightShadowIntent>(light)
            .unwrap()
            .0 = false;
        schedule.run(&mut world);
        assert!(
            !world
                .get::<bevy::pbr::ExtractedPointLight>(light)
                .unwrap()
                .shadow_maps_enabled
        );
        world
            .get_mut::<super::LocalLightShadowIntent>(light)
            .unwrap()
            .0 = true;
        schedule.run(&mut world);
        assert!(
            world
                .get::<bevy::pbr::ExtractedPointLight>(light)
                .unwrap()
                .shadow_maps_enabled
        );
        world.entity_mut(camera).insert(RenderLayers::layer(1));
        schedule.run(&mut world);
        assert!(
            !world
                .get::<bevy::pbr::ExtractedPointLight>(light)
                .unwrap()
                .shadow_maps_enabled
        );
        world.despawn(camera);
        schedule.run(&mut world);
        assert!(
            world
                .get::<bevy::pbr::ExtractedPointLight>(light)
                .unwrap()
                .shadow_maps_enabled
        );
    }

    #[test]
    fn extracted_light_removal_retires_only_absent_light_view_trackers() {
        let mut world = bevy::prelude::World::new();
        world.add_observer(super::retire_removed_local_light_shadow_views);
        let light = world.spawn(extracted_point_light(Vec3::ZERO, 20.0)).id();
        let view = world.spawn_empty().id();
        world
            .get_mut::<super::PointAndSpotLightViewEntities>(light)
            .unwrap()
            .push(view);
        // A queued retirement must not discard a light already re-extracted.
        super::retire_absent_local_light_shadow_views(&mut world, light);
        assert_eq!(
            world
                .get::<super::PointAndSpotLightViewEntities>(light)
                .unwrap()
                .len(),
            1
        );
        world
            .entity_mut(light)
            .remove::<bevy::pbr::ExtractedPointLight>();
        world.flush();
        assert!(
            world
                .get::<super::PointAndSpotLightViewEntities>(light)
                .is_none()
        );
        world
            .entity_mut(light)
            .insert(extracted_point_light(Vec3::ZERO, 20.0));
        assert!(
            world
                .get::<super::PointAndSpotLightViewEntities>(light)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn invalid_point_light_range_keeps_its_shadow() {
        let light_bounds = point_light_influence_sphere(Vec3::ZERO, f32::INFINITY);
        let light_layers = RenderLayers::default();
        let camera_frustum = perspective_frustum(Vec3::ZERO, 30.0, 60.0_f32.to_radians());

        assert!(!local_light_shadow_is_irrelevant(
            light_bounds,
            &light_layers,
            [(&camera_frustum, &light_layers)],
        ));
    }

    #[test]
    fn disjoint_spotlight_bounds_allow_shadow_map_skip() {
        let spotlight = conservative_frustum_sphere(&perspective_frustum(
            Vec3::new(100.0, 0.0, 0.0),
            20.0,
            20.0_f32.to_radians(),
        ))
        .unwrap();
        let light_layers = RenderLayers::default();
        let camera_frustum = perspective_frustum(Vec3::ZERO, 30.0, 60.0_f32.to_radians());

        assert!(local_light_shadow_is_irrelevant(
            spotlight,
            &light_layers,
            [(&camera_frustum, &light_layers)],
        ));
    }

    #[test]
    fn disjoint_layers_make_a_spotlight_irrelevant_to_that_view() {
        let spotlight = conservative_frustum_sphere(&perspective_frustum(
            Vec3::ZERO,
            20.0,
            20.0_f32.to_radians(),
        ))
        .unwrap();
        let camera_frustum = perspective_frustum(Vec3::ZERO, 30.0, 60.0_f32.to_radians());
        let camera_layers = RenderLayers::layer(1);
        let light_layers = RenderLayers::default();

        assert!(camera_frustum.intersects_sphere(&spotlight, true));
        assert!(!light_layers.intersects(&camera_layers));
        assert!(local_light_shadow_is_irrelevant(
            spotlight,
            &light_layers,
            [(&camera_frustum, &camera_layers)],
        ));
    }

    #[test]
    fn tangent_spheres_are_kept_for_shadow_quality() {
        let frustum = axis_aligned_frustum();
        let tangent = Sphere {
            center: Vec3::new(-1.0, 0.0, -1.0).into(),
            radius: 1.0,
        };

        assert!(frustum_intersects_sphere_conservatively(&frustum, tangent));
    }

    #[test]
    fn malformed_light_or_camera_frusta_fail_open() {
        assert!(conservative_frustum_sphere(&Frustum::default()).is_none());

        let spotlight = conservative_frustum_sphere(&perspective_frustum(
            Vec3::ZERO,
            20.0,
            20.0_f32.to_radians(),
        ))
        .unwrap();
        assert!(frustum_intersects_sphere_conservatively(
            &Frustum::default(),
            spotlight,
        ));
    }
}
