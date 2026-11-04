//! Physical mass validation and reduction at the native body boundary.
//!
//! Geometry contributes once per attached collider to automatic properties.
//! Explicit computed properties keep their native/co-simulation owner, and
//! independently simulated descendants contribute nothing.
use avian3d::parry::{mass_properties::MassProperties, math::Pose};
use avian3d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::ecs::schedule::{ScheduleLabel, SingleThreadedExecutor};
use bevy::prelude::*;

/// Construct one finite physical contribution before native reduction.
/// Zero principal inertia is allowed for an explicit point-mass realization.
pub fn checked_mass_properties(
    center: bevy::math::DVec3,
    mass: f64,
    principal: bevy::math::DVec3,
    frame: bevy::math::DQuat,
) -> Result<MassProperties, &'static str> {
    if !center.is_finite()
        || !mass.is_finite()
        || mass <= 0.0
        || mass > f64::from(f32::MAX)
        || !mass.recip().is_finite()
        || !principal.is_finite()
        || !principal.cmpge(bevy::math::DVec3::ZERO).all()
        || principal.max_element() > f64::from(f32::MAX)
        || !principal
            .to_array()
            .into_iter()
            .all(|value| value == 0.0 || value.recip().is_finite())
        || !center.as_vec3().is_finite()
        || !frame.is_finite()
        || !frame.is_normalized()
    {
        return Err(
            "mass contribution requires native-range positive mass, finite COM, nonnegative principal inertia with finite reciprocals and a unit frame",
        );
    }
    Ok(MassProperties::with_principal_inertia_frame(
        center, mass, principal, frame,
    ))
}

/// Validate a complete dynamic-body candidate before publishing any native
/// mass, tensor or centre-of-mass component. Parry owns tensor reduction and
/// principal-frame extraction; this is the explicit native solver boundary.
pub fn native_mass_properties(
    properties: &MassProperties,
) -> Result<(Mass, AngularInertia, CenterOfMass), &'static str> {
    let mass = properties.mass() as f32;
    let principal = properties.principal_inertia().as_vec3();
    let frame = properties.principal_inertia_local_frame.as_quat();
    let center = properties.local_com.as_vec3();
    if !mass.is_finite()
        || mass <= 0.0
        || !mass.recip().is_finite()
        || !principal.is_finite()
        || !principal.cmpgt(Vec3::ZERO).all()
        || !principal.recip().is_finite()
        || !frame.is_finite()
        || !frame.is_normalized()
        || !center.is_finite()
    {
        return Err(
            "reduced body mass, inertia or centre of mass exceeds the native dynamic-body representation",
        );
    }
    Ok((
        Mass(mass),
        AngularInertia::new_with_local_frame(principal, frame),
        CenterOfMass(center),
    ))
}

/// Native collider cooking and body mass preparation, without advancing time.
/// The spatial bridge runs this boundary before initial query admission and
/// before each solver invocation, after supplying current collider transforms.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhysicsGeometryPreparation;

/// Admission consumers run after this set to observe prepared native geometry.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhysicsGeometryAdmission;

/// Configure the native physics group to share geometry preparation between
/// lifecycle admission and fixed integration. Native plugins retain all shape
/// cooking, override validation and mass recomputation mechanisms.
pub fn with_geometry_preparation(plugins: PluginGroupBuilder) -> PluginGroupBuilder {
    plugins
        .set(MassPropertyPlugin::new(PhysicsGeometryPreparation))
        .set(ColliderBackendPlugin::<Collider>::new(
            PhysicsGeometryPreparation,
        ))
        .add(PhysicsGeometryPreparationPlugin)
}

struct PhysicsGeometryPreparationPlugin;

impl Plugin for PhysicsGeometryPreparationPlugin {
    fn build(&self, app: &mut App) {
        use avian3d::dynamics::rigid_body::mass_properties::MassPropertySystems;
        app.init_schedule(PhysicsGeometryPreparation);
        // Run the ordered geometry passes in the invoking thread.
        app.edit_schedule(PhysicsGeometryPreparation, |schedule| {
            schedule.set_executor(SingleThreadedExecutor::new());
        });
        app.add_systems(
            PhysicsGeometryPreparation,
            update_body_mass_from_colliders
                .after(MassPropertySystems::UpdateComputedMassProperties),
        );
    }
}

/// Derive automatic properties from `RigidBodyColliders`; explicit computed
/// properties remain owned by native override preparation and co-simulation.
#[derive(Component)]
pub struct BodyMassFromColliders;

#[allow(clippy::type_complexity)]
pub fn update_body_mass_from_colliders(
    mut bodies: Query<
        (
            Entity,
            &RigidBodyColliders,
            Has<NoAutoMass>,
            Has<NoAutoAngularInertia>,
            Has<NoAutoCenterOfMass>,
            &mut ComputedMass,
            &mut ComputedAngularInertia,
            &mut ComputedCenterOfMass,
        ),
        (
            With<RigidBody>,
            With<BodyMassFromColliders>,
            Or<(
                Added<BodyMassFromColliders>,
                Changed<RigidBodyColliders>,
                Changed<ComputedMass>,
                Changed<ComputedAngularInertia>,
                Changed<ComputedCenterOfMass>,
            )>,
        ),
    >,
    shapes: Query<(
        &ColliderMassProperties,
        &ColliderTransform,
        Option<&crate::PhysicsOrderKey>,
        Has<Sensor>,
    )>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut holds: Option<ResMut<crate::PhysicsHolds>>,
) {
    for (
        entity,
        colliders,
        no_auto_mass,
        no_auto_inertia,
        no_auto_center,
        mut computed_mass,
        mut computed_inertia,
        mut computed_center,
    ) in &mut bodies
    {
        if no_auto_mass && no_auto_inertia && no_auto_center {
            continue;
        }
        let order = crate::ordered_physics_entities(
            colliders.iter().map(|entity| {
                (
                    entity,
                    shapes.get(entity).ok().and_then(|(_, _, key, _)| key),
                )
            }),
            "body collider mass reduction",
        );
        let order = match order {
            Ok(order) => order,
            Err(invalid) => {
                crate::report_invalid_physics_order(
                    holds.as_deref_mut(),
                    faults.as_deref_mut(),
                    None,
                    "physics-body-mass-order-invalid",
                    &invalid,
                );
                continue;
            }
        };
        let geometry = order.into_iter().filter_map(|entity| {
            let (mass, transform, _, sensor) =
                shapes.get(entity).expect("validated collider mass input");
            (!sensor).then_some((*mass, *transform))
        });
        let mut properties = aggregate_collider_mass(geometry);
        if no_auto_mass {
            properties.set_mass(f64::from(computed_mass.value()), !no_auto_inertia);
        }
        let valid = properties.mass().is_finite()
            && properties.mass() >= 0.0
            && properties.mass() <= f64::from(f32::MAX)
            && crate::avian_backend_vector_is_valid(properties.principal_inertia())
            && crate::avian_backend_point_is_valid(properties.local_com)
            && crate::avian_backend_rotation_is_valid(properties.principal_inertia_local_frame);
        if !valid {
            if let Some(holds) = holds.as_deref_mut() {
                holds.set(crate::PhysicsHolds::SAFETY_FAILURE, true);
            }
            if let Some(faults) = faults.as_deref_mut() {
                faults.raise(
                    "physics-body-mass-invalid",
                    Some(entity),
                    "body collider mass",
                    "computed mass properties exceed the native solver representation",
                );
            }
            continue;
        }
        // Explicit Avian 0.8 solver boundary; the geometry reduction remains f64.
        if !no_auto_mass {
            let target_mass = properties.mass() as f32;
            if computed_mass.value() != target_mass {
                computed_mass.set(target_mass);
            }
        }
        if !no_auto_inertia {
            computed_inertia.set_if_neq(ComputedAngularInertia::new_with_local_frame(
                properties.principal_inertia().as_vec3(),
                properties.principal_inertia_local_frame.as_quat(),
            ));
        }
        if !no_auto_center {
            computed_center.set_if_neq(ComputedCenterOfMass(properties.local_com.as_vec3()));
        }
    }
}

fn aggregate_collider_mass(
    parts: impl Iterator<Item = (ColliderMassProperties, ColliderTransform)>,
) -> MassProperties {
    parts
        .map(|(mass, transform)| {
            // Avian publishes intrinsic shape mass through its f32 component. Widen
            // that native result once; body-local transforms and reduction stay f64.
            MassProperties::with_principal_inertia_frame(
                mass.center_of_mass.as_dvec3(),
                f64::from(mass.mass),
                mass.principal_angular_inertia.as_dvec3(),
                mass.local_inertial_frame.as_dquat(),
            )
            .transform_by(&Pose::from_parts(
                transform.translation.as_dvec3(),
                transform.rotation.as_dquat(),
            ))
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduced_mass_preserves_rotated_tensors_and_off_axis_centres() {
        use bevy::math::{DMat3, DQuat, DVec3};
        let base =
            checked_mass_properties(DVec3::ZERO, 1.0, DVec3::splat(10.0), DQuat::IDENTITY).unwrap();
        let child = checked_mass_properties(
            DVec3::ZERO,
            1.0,
            DVec3::new(1.0, 2.0, 3.0),
            DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2),
        )
        .unwrap();
        let reduced: MassProperties = [base, child].into_iter().sum();
        assert!(
            reduced
                .reconstruct_inertia_matrix()
                .abs_diff_eq(DMat3::from_diagonal(DVec3::new(12.0, 11.0, 13.0)), 1e-12)
        );
        assert!(native_mass_properties(&reduced).is_ok());

        let base =
            checked_mass_properties(DVec3::ZERO, 2.0, DVec3::new(1.0, 2.0, 3.0), DQuat::IDENTITY)
                .unwrap();
        let child = checked_mass_properties(
            DVec3::new(2.0, 2.0, 0.0),
            2.0,
            DVec3::new(1.0, 2.0, 3.0),
            DQuat::IDENTITY,
        )
        .unwrap();
        let reduced: MassProperties = [base, child].into_iter().sum();
        assert_eq!(reduced.local_com, DVec3::new(1.0, 1.0, 0.0));
        assert!(reduced.reconstruct_inertia_matrix().abs_diff_eq(
            DMat3::from_cols(
                DVec3::new(6.0, -4.0, 0.0),
                DVec3::new(-4.0, 8.0, 0.0),
                DVec3::new(0.0, 0.0, 14.0)
            ),
            1e-12
        ));
    }

    #[test]
    fn invalid_reduced_mass_is_rejected_before_native_construction() {
        use bevy::math::{DQuat, DVec3};
        assert!(
            checked_mass_properties(
                DVec3::ZERO,
                1.0,
                DVec3::new(1.0, -1.0, 1.0),
                DQuat::IDENTITY
            )
            .is_err()
        );
        assert!(
            checked_mass_properties(
                DVec3::splat(f64::INFINITY),
                1.0,
                DVec3::ONE,
                DQuat::IDENTITY
            )
            .is_err()
        );
        assert!(
            native_mass_properties(&MassProperties::new(DVec3::ZERO, 1.0, DVec3::ZERO)).is_err()
        );
        assert!(
            native_mass_properties(&MassProperties::new(DVec3::ZERO, 1.0, DVec3::splat(1e40)))
                .is_err()
        );
    }

    #[test]
    fn body_override_rescales_only_the_attached_geometry() {
        let cube = ColliderMassProperties::from_shape(&Cuboid::new(1.0, 1.0, 1.0), 1.0);
        let mut mass = aggregate_collider_mass([-1.0, 1.0].into_iter().map(|x| {
            (
                cube,
                ColliderTransform {
                    translation: Vec3::new(x, 0.0, 0.0),
                    ..default()
                },
            )
        }));
        assert_eq!(mass.mass(), 2.0);
        assert_eq!(mass.local_com, bevy::math::DVec3::ZERO);
        mass.set_mass(10.0, true);
        assert_eq!(mass.mass(), 10.0);
        let tensor = mass.reconstruct_inertia_matrix();
        assert!((tensor.x_axis.x - 10.0 / 6.0).abs() < 1e-5);
        assert!((tensor.y_axis.y - 70.0 / 6.0).abs() < 1e-5);
        assert!((tensor.z_axis.z - 70.0 / 6.0).abs() < 1e-5);
    }
}
