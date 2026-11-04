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

/// Standard per-shape mass facts retained independently of collider cooking.
/// The body geometry owner applies them before reducing attached shapes.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ColliderMassOverrides {
    pub mass: Option<f64>,
    pub principal: Option<bevy::math::DVec3>,
    pub principal_axes: Option<bevy::math::DQuat>,
    pub center: Option<bevy::math::DVec3>,
    /// A collider's axes rotate its derived tensor; a body's axes replace the
    /// principal frame of its already-reduced moments.
    pub rotate_geometry_frame: bool,
}

impl ColliderMassOverrides {
    /// Apply USD's independent mass, tensor, principal-frame and COM overrides.
    /// Parry owns mass rescaling and tensor diagonalization. Its public API
    /// does not expose a double-precision COM shift, so apply the parallel-axis
    /// term in the canonical tensor before asking Parry to diagonalize it.
    pub fn apply(self, mut geometry: MassProperties) -> Result<MassProperties, &'static str> {
        if geometry.mass() == 0.0 && self.mass.is_none() {
            return Ok(geometry);
        }
        if let Some(mass) = self.mass {
            if !mass.is_finite() || mass <= 0.0 {
                return Err("mass override must be finite and positive");
            }
            geometry.set_mass(mass, self.principal.is_none());
        }
        let mass = geometry.mass();
        let principal = self
            .principal
            .unwrap_or_else(|| geometry.principal_inertia());
        let frame = if self.principal.is_some() {
            self.principal_axes.unwrap_or(bevy::math::DQuat::IDENTITY)
        } else if let Some(axes) = self.principal_axes {
            if self.rotate_geometry_frame {
                axes * geometry.principal_inertia_local_frame
            } else {
                axes
            }
        } else {
            geometry.principal_inertia_local_frame
        };
        let mut result = checked_mass_properties(geometry.local_com, mass, principal, frame)?;
        if let Some(center) = self.center {
            if !center.is_finite() {
                return Err("centre-of-mass override must be finite");
            }
            if self.principal.is_none() {
                let offset = center - result.local_com;
                let outer = bevy::math::DMat3::from_cols(
                    offset * offset.x,
                    offset * offset.y,
                    offset * offset.z,
                );
                let shifted = result.reconstruct_inertia_matrix()
                    + (bevy::math::DMat3::from_diagonal(bevy::math::DVec3::splat(
                        offset.length_squared(),
                    )) - outer)
                        * mass;
                result = conditioned_tensor_properties(center, mass, shifted)?;
            } else {
                result.local_com = center;
            }
        }
        Ok(result)
    }
}

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
    let inertia = AngularInertia::new_with_local_frame(principal, frame);
    computed_inertia_tensor_d(&ComputedAngularInertia::new_with_local_frame(
        principal, frame,
    ))?;
    Ok((Mass(mass), inertia, CenterOfMass(center)))
}

/// Read the native owner's widened tensor and validate dynamic-body inertia.
pub fn computed_inertia_tensor_d(
    inertia: &ComputedAngularInertia,
) -> Result<bevy::math::DMat3, &'static str> {
    let tensor = bevy::math::DMat3::from(inertia.tensor_f64());
    if !tensor.is_finite()
        || tensor.x_axis.x <= 0.0
        || tensor.x_axis.x * tensor.y_axis.y - tensor.x_axis.y.powi(2) <= 0.0
        || tensor.determinant() <= 0.0
    {
        return Err("native inertia must be finite and positive definite");
    }
    Ok(tensor)
}

/// Principal facts read from the native inverse through the canonical f64 seam.
pub fn computed_inertia_principal_d(
    inertia: &ComputedAngularInertia,
) -> Result<(bevy::math::DVec3, bevy::math::DQuat), &'static str> {
    let tensor = computed_inertia_tensor_d(inertia)?;
    let properties = conditioned_tensor_properties(bevy::math::DVec3::ZERO, 1.0, tensor)?;
    Ok((
        properties.principal_inertia(),
        properties.principal_inertia_local_frame,
    ))
}

fn conditioned_tensor_properties(
    center: bevy::math::DVec3,
    mass: f64,
    tensor: bevy::math::DMat3,
) -> Result<MassProperties, &'static str> {
    let scale = tensor
        .to_cols_array()
        .into_iter()
        .map(f64::abs)
        .fold(0.0_f64, f64::max);
    if !scale.is_finite() {
        return Err("inertia tensor is non-finite");
    }
    let scale = if scale == 0.0 { 1.0 } else { scale };
    let normalized = MassProperties::with_inertia_matrix(center, mass, tensor / scale);
    Ok(MassProperties::with_principal_inertia_frame(
        center,
        mass,
        normalized.principal_inertia() * scale,
        normalized.principal_inertia_local_frame,
    ))
}

/// Condition moment units before maintained Parry reduction. Its eigenvector
/// extraction uses an absolute near-zero threshold; common mass/inertia
/// rescaling preserves COM and makes the principal frame independent of units.
pub fn sum_mass_properties(
    parts: impl IntoIterator<Item = MassProperties>,
) -> Result<MassProperties, &'static str> {
    let parts: Vec<_> = parts.into_iter().collect();
    let max_mass = parts
        .iter()
        .map(MassProperties::mass)
        .fold(0.0_f64, f64::max);
    if max_mass == 0.0 {
        return Ok(parts.into_iter().sum());
    }
    let weight: f64 = parts.iter().map(|part| part.mass() / max_mass).sum();
    let center = parts
        .iter()
        .map(|part| part.local_com * (part.mass() / max_mass))
        .sum::<bevy::math::DVec3>()
        / weight;
    let scale = parts
        .iter()
        .map(|part| {
            part.principal_inertia().max_element()
                + part.mass() * (part.local_com - center).length_squared()
        })
        .fold(0.0_f64, f64::max);
    if !scale.is_finite()
        || (scale != 0.0 && parts.iter().any(|part| !(part.mass() / scale).is_finite()))
    {
        return Err("mass-property moment normalization exceeds f64 range");
    }
    if scale == 0.0 {
        return Ok(parts.into_iter().sum());
    }
    let normalized: MassProperties = parts
        .into_iter()
        .map(|part| {
            MassProperties::with_principal_inertia_frame(
                part.local_com,
                part.mass() / scale,
                part.principal_inertia() / scale,
                part.principal_inertia_local_frame,
            )
        })
        .sum();
    Ok(MassProperties::with_principal_inertia_frame(
        normalized.local_com,
        normalized.mass() * scale,
        normalized.principal_inertia() * scale,
        normalized.principal_inertia_local_frame,
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
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BodyMassFromColliders {
    /// Derive mass from attached geometry; otherwise a native cache or computed input owns it.
    pub mass: bool,
    /// Derive inertia from geometry at the effective mass.
    pub inertia: bool,
    /// Derive the centre of mass from attached geometry.
    pub center: bool,
}

#[allow(clippy::type_complexity)]
pub fn update_body_mass_from_colliders(
    mut commands: Commands,
    mut bodies: Query<
        (
            Entity,
            Ref<RigidBodyColliders>,
            Ref<BodyMassFromColliders>,
            Option<&mut Mass>,
            Option<&mut AngularInertia>,
            Option<&mut CenterOfMass>,
            &mut ComputedMass,
            &mut ComputedAngularInertia,
            &mut ComputedCenterOfMass,
            Option<&ColliderMassOverrides>,
        ),
        With<RigidBody>,
    >,
    shapes: Query<(
        &Collider,
        &ColliderDensity,
        &ColliderTransform,
        Option<&crate::PhysicsOrderKey>,
        Has<Sensor>,
        Option<&ColliderMassOverrides>,
    )>,
    changed_shapes: Query<
        Entity,
        Or<(
            Changed<Collider>,
            Changed<ColliderDensity>,
            Changed<ColliderTransform>,
            Changed<ColliderMassOverrides>,
        )>,
    >,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut holds: Option<ResMut<crate::PhysicsHolds>>,
) {
    for (
        entity,
        colliders,
        automatic,
        mass_cache,
        inertia_cache,
        center_cache,
        mut computed_mass,
        mut computed_inertia,
        mut computed_center,
        body_overrides,
    ) in &mut bodies
    {
        if !automatic.mass && !automatic.inertia && !automatic.center {
            continue;
        }
        if !automatic.is_changed()
            && !colliders.is_changed()
            && !computed_mass.is_changed()
            && !computed_inertia.is_changed()
            && !computed_center.is_changed()
            && !colliders
                .iter()
                .any(|entity| changed_shapes.contains(entity))
        {
            continue;
        }
        let order = crate::ordered_physics_entities(
            colliders.iter().map(|entity| {
                (
                    entity,
                    shapes.get(entity).ok().and_then(|(_, _, _, key, _, _)| key),
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
            let (collider, density, transform, _, sensor, overrides) =
                shapes.get(entity).expect("validated collider mass input");
            (!sensor).then(|| {
                (
                    collider
                        .shape_scaled()
                        .mass_properties(f64::from(density.0)),
                    *transform,
                    overrides.copied().unwrap_or_default(),
                )
            })
        });
        let candidate = aggregate_collider_mass(geometry)
            .and_then(|properties| {
                let (principal, frame) = if let Some(inertia) = inertia_cache.as_deref() {
                    (inertia.principal.as_dvec3(), inertia.local_frame.as_dquat())
                } else {
                    computed_inertia_principal_d(&computed_inertia)?
                };
                ColliderMassOverrides {
                    mass: (!automatic.mass).then(|| {
                        mass_cache
                            .as_deref()
                            .map(|mass| f64::from(mass.0))
                            .unwrap_or_else(|| f64::from(computed_mass.value()))
                    }),
                    principal: (!automatic.inertia).then_some(principal),
                    principal_axes: if !automatic.inertia {
                        Some(frame)
                    } else {
                        body_overrides.and_then(|overrides| overrides.principal_axes)
                    },
                    center: (!automatic.center).then(|| {
                        center_cache
                            .as_deref()
                            .map(|center| center.0.as_dvec3())
                            .unwrap_or_else(|| computed_center.0.as_dvec3())
                    }),
                    ..default()
                }
                .apply(properties)
            })
            .and_then(|properties| native_mass_properties(&properties));
        let (mass, inertia, center) = match candidate {
            Ok(candidate) => candidate,
            Err(detail) => {
                if let Some(holds) = holds.as_deref_mut() {
                    holds.set(crate::PhysicsHolds::SAFETY_FAILURE, true);
                }
                if let Some(faults) = faults.as_deref_mut() {
                    faults.raise(
                        "physics-body-mass-invalid",
                        Some(entity),
                        "body collider mass",
                        detail,
                    );
                }
                continue;
            }
        };
        // Native observers consume only these validated caches. The automatic
        // owner remains active and republishes after geometry/source changes.
        if automatic.mass {
            if let Some(mut cache) = mass_cache {
                cache.set_if_neq(mass);
            } else {
                commands.entity(entity).try_insert((mass, NoAutoMass));
            }
            computed_mass.set_if_neq(ComputedMass::new(mass.0));
        }
        if automatic.inertia {
            if let Some(mut cache) = inertia_cache {
                cache.set_if_neq(inertia);
            } else {
                commands
                    .entity(entity)
                    .try_insert((inertia, NoAutoAngularInertia));
            }
            computed_inertia.set_if_neq(ComputedAngularInertia::new_with_local_frame(
                inertia.principal,
                inertia.local_frame,
            ));
        }
        if automatic.center {
            if let Some(mut cache) = center_cache {
                cache.set_if_neq(center);
            } else {
                commands
                    .entity(entity)
                    .try_insert((center, NoAutoCenterOfMass));
            }
            computed_center.set_if_neq(ComputedCenterOfMass(center.0));
        }
    }
}

fn aggregate_collider_mass(
    parts: impl Iterator<Item = (MassProperties, ColliderTransform, ColliderMassOverrides)>,
) -> Result<MassProperties, &'static str> {
    parts
        .map(|(properties, transform, overrides)| {
            Ok(overrides.apply(properties)?.transform_by(&Pose::from_parts(
                transform.translation.as_dvec3(),
                transform.rotation.as_dquat(),
            )))
        })
        .collect::<Result<Vec<_>, _>>()
        .and_then(sum_mass_properties)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computed_inverse_tensor_reads_remain_finite_across_native_determinant_limits() {
        for principal in [1e-17_f32, 1e20_f32] {
            let native = ComputedAngularInertia::new(Vec3::splat(principal));
            let tensor = computed_inertia_tensor_d(&native).unwrap();
            for value in tensor.diagonal().to_array() {
                assert!((value / f64::from(principal) - 1.0).abs() < 1e-6);
            }
        }
        assert!(computed_inertia_tensor_d(&ComputedAngularInertia::INFINITY).is_err());
    }

    #[test]
    fn reduced_principal_frame_is_independent_of_mass_and_length_scale() {
        use bevy::math::{DQuat, DVec3};
        let sources = |mass_scale: f64, length_scale: f64| {
            [
                MassProperties::with_principal_inertia_frame(
                    DVec3::ZERO,
                    mass_scale,
                    DVec3::new(1.0, 2.0, 3.0) * mass_scale * length_scale * length_scale,
                    DQuat::from_rotation_z(0.61),
                ),
                MassProperties::with_principal_inertia_frame(
                    DVec3::new(1.0, 2.0, 0.0) * length_scale,
                    2.0 * mass_scale,
                    DVec3::new(2.0, 1.0, 3.0) * mass_scale * length_scale * length_scale,
                    DQuat::from_rotation_y(0.43),
                ),
            ]
        };
        let normal = sum_mass_properties(sources(1.0, 1.0)).unwrap();
        let small = sum_mass_properties(sources(1e10, 1e-13)).unwrap();
        assert!((small.mass() / 1e10 - normal.mass()).abs() < 1e-12);
        assert!((small.local_com / 1e-13).abs_diff_eq(normal.local_com, 1e-12));
        assert!(
            (small.reconstruct_inertia_matrix() / 1e-16)
                .abs_diff_eq(normal.reconstruct_inertia_matrix(), 1e-10)
        );
    }

    #[test]
    fn authored_shape_mass_tensor_and_com_override_geometry_independently() {
        use bevy::math::{DMat3, DQuat, DVec3};
        let geometry = checked_mass_properties(
            DVec3::ZERO,
            100.0,
            DVec3::splat(100.0 / 6.0),
            DQuat::IDENTITY,
        )
        .unwrap();
        let child = ColliderMassOverrides {
            mass: Some(7.0),
            principal: Some(DVec3::new(1.0, 2.0, 3.0)),
            principal_axes: Some(DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2)),
            center: Some(DVec3::new(2.0, 0.0, 0.0)),
            ..default()
        }
        .apply(geometry)
        .unwrap();
        assert_eq!(child.mass(), 7.0);
        assert_eq!(child.local_com, DVec3::new(2.0, 0.0, 0.0));
        assert!(
            child
                .reconstruct_inertia_matrix()
                .abs_diff_eq(DMat3::from_diagonal(DVec3::new(2.0, 1.0, 3.0)), 1e-12)
        );
        let root = ColliderMassOverrides {
            mass: Some(14.0),
            center: Some(DVec3::ZERO),
            ..default()
        }
        .apply(child)
        .unwrap();
        assert_eq!(root.mass(), 14.0);
        assert!(
            root.reconstruct_inertia_matrix()
                .abs_diff_eq(DMat3::from_diagonal(DVec3::new(4.0, 58.0, 62.0)), 1e-10)
        );
    }

    #[test]
    fn collider_axes_rotate_derived_geometry_frame_without_replacing_it() {
        use bevy::math::{DMat3, DQuat, DVec3};
        let frame = DQuat::from_rotation_y(0.37);
        let axes = DQuat::from_rotation_z(0.61);
        let geometry =
            checked_mass_properties(DVec3::ZERO, 2.0, DVec3::new(1.0, 2.0, 3.0), frame).unwrap();
        let rotated = ColliderMassOverrides {
            principal_axes: Some(axes),
            rotate_geometry_frame: true,
            ..default()
        }
        .apply(geometry)
        .unwrap();
        let rotation = DMat3::from_quat(axes);
        let expected = rotation * geometry.reconstruct_inertia_matrix() * rotation.transpose();
        assert!(
            rotated
                .reconstruct_inertia_matrix()
                .abs_diff_eq(expected, 1e-12)
        );
    }

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
        let cube = Collider::cuboid(1.0, 1.0, 1.0)
            .shape_scaled()
            .mass_properties(1.0);
        let mut mass = aggregate_collider_mass([-1.0, 1.0].into_iter().map(|x| {
            (
                cube,
                ColliderTransform {
                    translation: Vec3::new(x, 0.0, 0.0),
                    ..default()
                },
                ColliderMassOverrides::default(),
            )
        }))
        .unwrap();
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
