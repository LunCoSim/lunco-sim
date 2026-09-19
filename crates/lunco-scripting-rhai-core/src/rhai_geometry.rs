//! Native f64 geometry and profile construction for Rhai.
//!
//! The values are the same Bevy/glam vectors, quaternions, and transforms used
//! by the simulator. USD-facing arrays are created only when an authored plan
//! explicitly lowers these types at the stage boundary.

use bevy::math::{DQuat, DVec2, DVec3};
use lunco_core::DTransform;
use lunco_geometry_core::bounds::{Bounds3, BoundsRelation, OrientedBounds3};
use lunco_geometry_core::profile_extrusion::{
    ProfileExtrusionError, ProfileMeshData, ProfilePlane, extrude_profile,
};
use rhai::{Array, Dynamic, Engine, EvalAltResult, Position};

fn runtime_error(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(message.into().into(), Position::NONE).into()
}

fn bounds(min: DVec3, max: DVec3) -> Result<Bounds3, Box<EvalAltResult>> {
    Bounds3::new(min, max)
        .ok_or_else(|| runtime_error("Bounds3 requires finite corners ordered min <= max"))
}

fn oriented_bounds(
    center: DVec3,
    half_extents: DVec3,
    rotation: DQuat,
) -> Result<OrientedBounds3, Box<EvalAltResult>> {
    OrientedBounds3::new(center, half_extents, rotation).ok_or_else(|| {
        runtime_error("OrientedBounds3 requires finite center, extents, and rotation")
    })
}

fn oriented_bounds_from_local(
    local: Bounds3,
    pose: DTransform,
) -> Result<OrientedBounds3, Box<EvalAltResult>> {
    if !pose.is_finite() {
        return Err(runtime_error(
            "local bounds cannot be transformed by this pose",
        ));
    }
    let center = pose
        .transform_point(local.center())
        .ok_or_else(|| runtime_error("local bounds center cannot be transformed by this pose"))?;
    OrientedBounds3::new(
        center,
        local.half_extents() * pose.scale.abs(),
        pose.rotation,
    )
    .ok_or_else(|| runtime_error("local bounds cannot be transformed by this pose"))
}

fn profile_points(values: Array) -> Result<Vec<DVec2>, Box<EvalAltResult>> {
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .try_cast::<DVec2>()
                .filter(|point| point.is_finite())
                .ok_or_else(|| {
                    runtime_error(format!("profile point {index} must be a finite Vec2"))
                })
        })
        .collect()
}

fn profile_extrusion(
    values: Array,
    length: f64,
    plane: ProfilePlane,
) -> Result<ProfileMeshData, Box<EvalAltResult>> {
    let points = profile_points(values)?;
    extrude_profile(&points, length, plane).map_err(|error: ProfileExtrusionError| {
        runtime_error(format!("profile extrusion failed: {error}"))
    })
}

fn vec3_array(values: &[DVec3]) -> Array {
    values.iter().copied().map(Dynamic::from).collect()
}

fn int_array(values: &[i32]) -> Array {
    values
        .iter()
        .map(|value| Dynamic::from_int(i64::from(*value)))
        .collect()
}

/// Register geometry constructors and accessors on an existing Rhai engine.
/// `rhai_math::register` invokes this after registering Vec2/Vec3/Quat/Transform.
pub fn register(engine: &mut Engine) {
    engine
        .register_type_with_name::<Bounds3>("Bounds3")
        .register_get("min", |value: &mut Bounds3| value.min())
        .register_get("max", |value: &mut Bounds3| value.max())
        .register_get("center", |value: &mut Bounds3| value.center())
        .register_get("half_extents", |value: &mut Bounds3| value.half_extents())
        .register_type_with_name::<OrientedBounds3>("OrientedBounds3")
        .register_get("center", |value: &mut OrientedBounds3| value.center())
        .register_get("half_extents", |value: &mut OrientedBounds3| {
            value.half_extents()
        })
        .register_get("rotation", |value: &mut OrientedBounds3| value.rotation())
        .register_type_with_name::<BoundsRelation>("BoundsRelation")
        .register_get("greatest_axis_margin", |value: &mut BoundsRelation| {
            value.greatest_axis_margin
        })
        .register_get("axis_from_a_to_b", |value: &mut BoundsRelation| {
            value.axis_from_a_to_b
        })
        .register_get("separated", |value: &mut BoundsRelation| value.separated)
        .register_type_with_name::<ProfilePlane>("ProfilePlane")
        .register_type_with_name::<ProfileMeshData>("ProfileMesh")
        .register_get("points", |mesh: &mut ProfileMeshData| {
            vec3_array(mesh.points())
        })
        .register_get("face_vertex_counts", |mesh: &mut ProfileMeshData| {
            int_array(mesh.face_vertex_counts())
        })
        .register_get("face_vertex_indices", |mesh: &mut ProfileMeshData| {
            int_array(mesh.face_vertex_indices())
        })
        .register_get("face_varying_normals", |mesh: &mut ProfileMeshData| {
            vec3_array(mesh.face_varying_normals())
        })
        .register_get("vertex_count", |mesh: &mut ProfileMeshData| {
            mesh.vertex_count() as i64
        })
        .register_get("face_count", |mesh: &mut ProfileMeshData| {
            mesh.face_count() as i64
        })
        .register_fn("bounds3", bounds)
        .register_fn("oriented_bounds3", oriented_bounds)
        .register_fn("oriented_bounds_from_local", oriented_bounds_from_local)
        .register_fn(
            "bounds_relation",
            |a: OrientedBounds3, b: OrientedBounds3| a.relation(b),
        )
        .register_fn("profile_plane_xy", || ProfilePlane::XY)
        .register_fn("profile_plane_xz", || ProfilePlane::XZ)
        .register_fn("profile_plane_yz", || ProfilePlane::YZ)
        .register_fn("extrude_profile", profile_extrusion);
}
