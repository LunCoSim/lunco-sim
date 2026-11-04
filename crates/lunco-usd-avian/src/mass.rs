//! Standard MassAPI facts at the composed USD/native-physics boundary.

use avian3d::parry::mass_properties::MassProperties;
use bevy::math::{DQuat, DVec3};
use lunco_physics::body_mass::ColliderMassOverrides;
use lunco_usd_bevy_stage::read::UsdReadObject;
use openusd::sdf::Path;
use openusd_schemas::physics::tokens as ptok;

pub(crate) fn overrides(
    reader: &dyn UsdReadObject,
    path: &Path,
) -> Result<ColliderMassOverrides, String> {
    let convention =
        lunco_usd_bevy_stage::stage_convention(reader).map_err(|error| error.to_string())?;
    let length = convention.length(1.0);
    let mass = match lunco_usd_avian_reader::read_authored_real(reader, path, ptok::PHYSICS_MASS)
        .map_err(|()| "physics:mass has an unreadable authored type".to_owned())?
    {
        None | Some(0.0) => None,
        Some(value) => {
            let native = value as f32;
            if !value.is_finite()
                || native <= 0.0
                || !native.is_finite()
                || !native.recip().is_finite()
            {
                return Err(format!(
                    "physics:mass={value:?} must be positive and form a finite native value and reciprocal"
                ));
            }
            Some(value)
        }
    };
    let principal = match lunco_usd_avian_reader::read_authored_vec3(
        reader,
        path,
        ptok::PHYSICS_DIAGONAL_INERTIA,
    )
    .map_err(|()| "physics:diagonalInertia must contain three finite numbers".to_owned())?
    {
        None => None,
        Some(value) if value == DVec3::ZERO => None,
        Some(value) => {
            if !value.cmpgt(DVec3::ZERO).all() {
                return Err(format!(
                    "physics:diagonalInertia={value:?} requires positive principal moments or the all-zero automatic sentinel"
                ));
            }
            let value = convention.dir_d(value).abs() * length.powi(2);
            let native = value.as_vec3();
            if !native.is_finite()
                || !native.cmpgt(bevy::math::Vec3::ZERO).all()
                || !native.recip().is_finite()
            {
                return Err(format!(
                    "physics:diagonalInertia={value:?} kg m² cannot form finite positive native principal moments and reciprocals"
                ));
            }
            Some(value)
        }
    };
    let principal_axes = if reader.has_authored_attribute(path, ptok::PHYSICS_PRINCIPAL_AXES) {
        let q = reader
            .attr_value(path, ptok::PHYSICS_PRINCIPAL_AXES)
            .and_then(|value| value.get::<openusd::gf::Quatf>())
            .ok_or_else(|| "physics:principalAxes must be a quatf".to_owned())?;
        let q = DQuat::from_xyzw(
            f64::from(q.x),
            f64::from(q.y),
            f64::from(q.z),
            f64::from(q.w),
        );
        if q == DQuat::from_xyzw(0.0, 0.0, 0.0, 0.0) {
            None // MassAPI's documented omitted-principal-frame sentinel.
        } else if q.is_finite() && q.length_squared() > f64::EPSILON {
            Some(convention.rotation_d(q.normalize()))
        } else {
            return Err(format!(
                "physics:principalAxes={q:?} must be finite and normalizable, or the zero automatic sentinel"
            ));
        }
    } else {
        None
    };
    let center = center_of_mass(reader, path)?;
    Ok(ColliderMassOverrides {
        mass,
        principal,
        principal_axes,
        center,
        ..Default::default()
    })
}

/// Read a standard authored COM in the scale-free native prim frame.
/// World scale is applied once because native rigid poses carry no scale.
pub(crate) fn center_of_mass(
    reader: &dyn UsdReadObject,
    path: &Path,
) -> Result<Option<DVec3>, String> {
    let convention =
        lunco_usd_bevy_stage::stage_convention(reader).map_err(|error| error.to_string())?;
    let center = if reader.has_authored_attribute(path, ptok::PHYSICS_CENTER_OF_MASS) {
        let value = DVec3::from_array(
            reader
                .vec3_f64(path, ptok::PHYSICS_CENTER_OF_MASS)
                .ok_or_else(|| "physics:centerOfMass must be a three-component point".to_owned())?,
        );
        if value == DVec3::splat(f64::NEG_INFINITY) {
            None // Standard geometry-derived COM sentinel.
        } else if value.is_finite() {
            let world = lunco_usd_bevy_stage::world_transform_matrix_d_at(reader, path, 0.0)
                .map_err(|error| format!("physics:centerOfMass transform: {error}"))?;
            if world.determinant() == 0.0
                || !world.determinant().is_finite()
                || !world.row(3).abs_diff_eq(bevy::math::DVec4::W, 1e-6)
            {
                return Err(
                    "physics:centerOfMass requires a nonsingular affine prim transform".to_owned(),
                );
            }
            let (scale, _, _) = world.to_scale_rotation_translation();
            let center = convention.point_d(value) * scale;
            if !center.as_vec3().is_finite() {
                return Err(format!(
                    "physics:centerOfMass={value:?} with world scale {scale:?} exceeds the native range"
                ));
            }
            Some(center)
        } else {
            return Err(format!(
                "physics:centerOfMass={value:?} must be finite or the standard (-inf,-inf,-inf) automatic sentinel"
            ));
        }
    } else {
        None
    };
    Ok(center)
}

/// No collision volume means a unit mass unless explicitly authored. The
/// standard missing-inertia approximation is a sphere of radius 0.1 m.
pub(crate) fn body_candidate(
    geometry: MassProperties,
    overrides: ColliderMassOverrides,
) -> Result<MassProperties, &'static str> {
    let geometry = if geometry.mass() == 0.0 {
        let mass = overrides.mass.unwrap_or(1.0);
        lunco_physics::body_mass::checked_mass_properties(
            overrides.center.unwrap_or(DVec3::ZERO),
            mass,
            DVec3::splat(0.004 * mass),
            DQuat::IDENTITY,
        )?
    } else {
        geometry
    };
    overrides.apply(geometry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_usd_bevy_stage::UsdStageProjectionPlan;
    use lunco_usd_compose::recipe::StageRecipe;

    fn plan(attrs: &str) -> UsdStageProjectionPlan {
        let source = format!(
            "#usda 1.0\n(metersPerUnit = 1)\ndef Xform \"Body\" (prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysicsMassAPI\"]) {{\n{attrs}\n}}\n"
        );
        UsdStageProjectionPlan::from_recipe(&StageRecipe::from_source("inline_mass.usda", &source))
            .unwrap()
    }

    #[test]
    fn native_mass_parser_reports_tiny_and_negative_values_with_the_property() {
        let path = Path::new("/Body").unwrap();
        for value in ["1e-45", "1e-300", "-1"] {
            let reader = plan(&format!("double physics:mass = {value}"));
            let error = overrides(&reader, &path).unwrap_err();
            assert!(error.contains("physics:mass="), "{error}");
            assert!(error.contains("reciprocal"), "{error}");
        }
    }

    #[test]
    fn mass_zero_sentinels_retain_standard_density_and_scaled_com() {
        let path = Path::new("/Body").unwrap();
        let reader = plan(
            "float physics:mass = 0\nfloat physics:density = 0\nfloat3 physics:diagonalInertia = (0,0,0)\nquatf physics:principalAxes = (0,0,0,0)\npoint3f physics:centerOfMass = (1,2,3)\nfloat3 xformOp:scale = (2,3,4)\nuniform token[] xformOpOrder = [\"xformOp:scale\"]",
        );
        let facts = overrides(&reader, &path).unwrap();
        assert!(
            facts.mass.is_none() && facts.principal.is_none() && facts.principal_axes.is_none()
        );
        assert_eq!(facts.center, Some(DVec3::new(2.0, 6.0, 12.0)));
        assert_eq!(
            crate::collider_density_from_usd(&reader, &path, &path)
                .unwrap()
                .0,
            1000.0
        );
        let reader = plan("point3f physics:centerOfMass = (-inf,-inf,-inf)");
        assert!(overrides(&reader, &path).unwrap().center.is_none());
    }

    #[test]
    fn no_volume_mass_uses_the_standard_unit_and_small_sphere_inertia() {
        let properties =
            body_candidate(MassProperties::default(), ColliderMassOverrides::default()).unwrap();
        assert_eq!(properties.mass(), 1.0);
        assert!(
            properties
                .principal_inertia()
                .abs_diff_eq(DVec3::splat(0.004), 1e-12)
        );
        assert!(lunco_physics::body_mass::native_mass_properties(&properties).is_ok());
        let properties = body_candidate(
            MassProperties::default(),
            ColliderMassOverrides {
                center: Some(DVec3::X),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(properties.local_com, DVec3::X);
        assert!(
            properties
                .reconstruct_inertia_matrix()
                .abs_diff_eq(bevy::math::DMat3::from_diagonal(DVec3::splat(0.004)), 1e-12,)
        );
    }

    #[test]
    fn empty_body_native_admission_retains_its_mass_through_geometry_preparation() {
        use avian3d::prelude::*;
        use bevy::prelude::*;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin));
        app.add_plugins(lunco_physics::body_mass::with_geometry_preparation(
            PhysicsPlugins::default().build(),
        ));
        app.finish();
        let candidate = body_candidate(
            avian3d::parry::mass_properties::MassProperties::default(),
            ColliderMassOverrides::default(),
        )
        .unwrap();
        let (mass, inertia, center) =
            lunco_physics::body_mass::native_mass_properties(&candidate).unwrap();
        let entity = app
            .world_mut()
            .spawn((
                mass,
                inertia,
                center,
                NoAutoMass,
                NoAutoAngularInertia,
                NoAutoCenterOfMass,
                RigidBody::Kinematic,
                lunco_physics::body_mass::BodyMassFromColliders::default(),
            ))
            .id();
        for _ in 0..3 {
            app.world_mut()
                .run_schedule(lunco_physics::body_mass::PhysicsGeometryPreparation);
        }
        assert_eq!(
            app.world().get::<ComputedMass>(entity).unwrap().value(),
            1.0
        );
        let (principal, _) = app
            .world()
            .get::<ComputedAngularInertia>(entity)
            .unwrap()
            .principal_angular_inertia_with_local_frame();
        assert!(principal.abs_diff_eq(Vec3::splat(0.004), 1e-8));
    }

    #[test]
    fn checked_native_geometry_owner_shields_observers_and_keeps_automatic_inertia_live() {
        use avian3d::prelude::*;
        use bevy::prelude::*;
        for (scale, explicit_mass, invalid) in
            [(3e-9_f32, None, true), (1e-13_f32, Some(1e10_f64), false)]
        {
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, TransformPlugin));
            app.add_plugins(lunco_physics::body_mass::with_geometry_preparation(
                PhysicsPlugins::default().build(),
            ));
            app.init_resource::<lunco_core::RuntimeFaults>();
            app.init_resource::<lunco_physics::PhysicsHolds>();
            app.finish();
            // A valid admitted cache precedes an edited/scaled collider.
            let mass = explicit_mass.unwrap_or(1000.0) as f32;
            let body = app
                .world_mut()
                .spawn((
                    Mass(mass),
                    AngularInertia::new(Vec3::splat(mass / 6.0)),
                    CenterOfMass(Vec3::ZERO),
                    NoAutoMass,
                    NoAutoAngularInertia,
                    NoAutoCenterOfMass,
                    RigidBody::Kinematic,
                    lunco_physics::body_mass::BodyMassFromColliders {
                        mass: explicit_mass.is_none(),
                        inertia: true,
                        center: true,
                    },
                    Transform::IDENTITY,
                ))
                .id();
            let mut collider = Collider::cuboid(1.0, 1.0, 1.0);
            collider.set_scale(Vec3::splat(scale), 10);
            let shape = app
                .world_mut()
                .spawn((
                    collider,
                    ColliderDensity(1000.0),
                    ColliderOf { body },
                    ChildOf(body),
                    ColliderTransform {
                        scale: Vec3::splat(scale),
                        ..default()
                    },
                    Transform::from_scale(Vec3::splat(scale)),
                    lunco_physics::PhysicsOrderKey("inline/scaled-body/shape".to_owned()),
                ))
                .id();
            app.world_mut()
                .run_schedule(lunco_physics::body_mass::PhysicsGeometryPreparation);
            assert_eq!(
                app.world().resource::<lunco_core::RuntimeFaults>().active(),
                invalid
            );
            if invalid {
                assert!(
                    app.world()
                        .resource::<lunco_physics::PhysicsHolds>()
                        .holds(lunco_physics::PhysicsHolds::SAFETY_FAILURE)
                );
                assert_eq!(app.world().get::<Mass>(body).unwrap().0, mass);
            } else {
                let before = app.world().get::<AngularInertia>(body).unwrap().principal.x;
                let expected = mass * scale * scale / 6.0;
                assert!((before / expected - 1.0).abs() < 1e-5);
                app.world_mut()
                    .get_mut::<ColliderTransform>(shape)
                    .unwrap()
                    .scale = Vec3::splat(scale * 2.0);
                app.world_mut().get_mut::<Transform>(shape).unwrap().scale =
                    Vec3::splat(scale * 2.0);
                app.world_mut()
                    .run_schedule(lunco_physics::body_mass::PhysicsGeometryPreparation);
                let after = app.world().get::<AngularInertia>(body).unwrap().principal.x;
                assert!((after / before - 4.0).abs() < 1e-4);
                assert!(!app.world().resource::<lunco_core::RuntimeFaults>().active());
            }
        }
    }

    #[test]
    fn scaled_rotated_child_com_and_principal_axes_survive_root_mass_precedence() {
        let source = r#"#usda 1.0
(metersPerUnit = 1)
def Xform "Body" (prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsMassAPI"]) {
    float physics:mass = 14
    point3f physics:centerOfMass = (0,0,0)
    def Cube "Shape" (prepend apiSchemas = ["PhysicsCollisionAPI", "PhysicsMassAPI"]) {
        float physics:mass = 7
        float physics:density = 100
        float3 physics:diagonalInertia = (1,2,3)
        point3f physics:centerOfMass = (1,0,0)
        quatf physics:principalAxes = (0.70710677,0,0,0.70710677)
        double xformOp:rotateZ = 90
        float3 xformOp:scale = (2,3,4)
        uniform token[] xformOpOrder = ["xformOp:rotateZ", "xformOp:scale"]
    }
}
"#;
        let reader = UsdStageProjectionPlan::from_recipe(&StageRecipe::from_source(
            "inline_mass.usda",
            source,
        ))
        .unwrap();
        let root = Path::new("/Body").unwrap();
        let child = Path::new("/Body/Shape").unwrap();
        let unit = lunco_physics::body_mass::checked_mass_properties(
            DVec3::ZERO,
            100.0,
            DVec3::splat(10.0),
            DQuat::IDENTITY,
        )
        .unwrap();
        let properties = overrides(&reader, &child).unwrap().apply(unit).unwrap();
        assert_eq!(properties.mass(), 7.0);
        assert!(
            properties
                .local_com
                .abs_diff_eq(DVec3::new(2.0, 0.0, 0.0), 1e-12)
        );
        let mounted = properties.transform_by(&avian3d::parry::math::Pose::from_parts(
            DVec3::ZERO,
            DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2),
        ));
        let candidate = body_candidate(mounted, overrides(&reader, &root).unwrap()).unwrap();
        assert_eq!(candidate.mass(), 14.0);
        assert_eq!(candidate.local_com, DVec3::ZERO);
        assert!(candidate.reconstruct_inertia_matrix().abs_diff_eq(
            bevy::math::DMat3::from_diagonal(DVec3::new(58.0, 4.0, 62.0)),
            1e-5,
        ));
    }
}
