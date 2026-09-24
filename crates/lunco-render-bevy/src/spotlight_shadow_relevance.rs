//! Avoid preparing spotlight shadow views that cannot contribute to any output.
//!
//! Bevy shares each spotlight shadow map across cameras. Its normal path keeps
//! maps for visible lights even when the light's finite cone misses every active
//! 3D camera. This render-world adapter tests the authored light frustum against
//! every extracted 3D camera and suppresses only that light's shadow-map flag.
//! The main-world light, direct illumination, light range, and shadow quality
//! settings remain unchanged.

use bevy::{
    camera::{
        Camera3d,
        primitives::{Frustum, Sphere},
        visibility::RenderLayers,
    },
    pbr::ExtractedPointLight,
    prelude::{App, IntoScheduleConfigs, Query, With},
    render::{Render, RenderApp, RenderSystems, camera::ExtractedCamera},
};

pub(super) fn build(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };

    render_app.add_systems(
        Render,
        suppress_irrelevant_spotlight_shadows
            .in_set(RenderSystems::CreateViews)
            .before(bevy::pbr::prepare_lights),
    );
}

fn suppress_irrelevant_spotlight_shadows(
    mut lights: Query<(&mut ExtractedPointLight, &RenderLayers, Option<&Frustum>)>,
    cameras: Query<(&Frustum, Option<&RenderLayers>), (With<Camera3d>, With<ExtractedCamera>)>,
) {
    // Keep authored state intact if there is no extracted output view to judge
    // against. Extracted cameras are Bevy's active, renderable Camera3d views.
    if cameras.is_empty() {
        return;
    }

    let default_layers = RenderLayers::default();
    for (mut light, light_layers, light_frustum) in &mut lights {
        // Point-light cubemaps are not part of this optimization. Bevy encodes
        // spots as ExtractedPointLight with spot_light_angles populated.
        if !light.shadow_maps_enabled || light.spot_light_angles.is_none() {
            continue;
        }

        let Some(light_frustum) = light_frustum else {
            continue;
        };
        let Some(light_bounds) = conservative_frustum_sphere(light_frustum) else {
            continue;
        };

        let camera_views = cameras
            .iter()
            .map(|(frustum, layers)| (frustum, layers.unwrap_or(&default_layers)));
        let irrelevant_to_all_cameras =
            spotlight_shadow_is_irrelevant(light_bounds, light_layers, camera_views);

        if irrelevant_to_all_cameras {
            // This is render-world extracted state only. extract_lights refreshes
            // it from the authored SpotLight before the next render schedule.
            light.shadow_maps_enabled = false;
        }
    }
}

/// A sphere enclosing all corners of a finite light frustum.
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

fn spotlight_shadow_is_irrelevant<'a>(
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
        camera::{
            CameraProjection, PerspectiveProjection,
            primitives::{Frustum, Sphere},
            visibility::RenderLayers,
        },
        math::{
            Vec3,
            primitives::{HalfSpace, ViewFrustum},
        },
        prelude::{GlobalTransform, Transform},
    };

    use super::{
        conservative_frustum_sphere, frustum_intersects_sphere_conservatively,
        spotlight_shadow_is_irrelevant,
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
    fn spotlight_bounds_are_finite_and_tighter_than_the_point_range_sphere() {
        let spot_frustum = perspective_frustum(Vec3::ZERO, 90.0, 20.0_f32.to_radians());
        let bounds = conservative_frustum_sphere(&spot_frustum).unwrap();

        assert!(bounds.radius.is_finite());
        assert!(bounds.radius < 90.0);
        for corner in spot_frustum.0.corners().unwrap() {
            assert!(bounds.center.distance(corner.into()) <= bounds.radius);
        }
    }

    #[test]
    fn any_matching_camera_view_keeps_the_spotlight_shadow() {
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

        assert!(!spotlight_shadow_is_irrelevant(
            spotlight,
            &light_layers,
            [
                (&offscreen_view, &light_layers),
                (&receiving_view, &light_layers)
            ],
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

        assert!(spotlight_shadow_is_irrelevant(
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
        assert!(spotlight_shadow_is_irrelevant(
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
