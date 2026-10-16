//! Implicit coordinate drives evaluated after free position integration.

use avian3d::dynamics::solver::{
    solver_body::{SolverBody, SolverBodyInertia},
    xpbd::AngularConstraint,
};
use avian3d::prelude::MotorModel;
use bevy::math::DVec3;

/// Dimensional or mass-normalized law for one generalized coordinate.
#[derive(Clone, Copy, Debug)]
pub struct CoordinateDrive {
    pub model: MotorModel,
    pub target_position: f64,
    pub target_velocity: f64,
    /// Physical force/torque cap, including for acceleration-based laws.
    pub max_force: f64,
}

impl CoordinateDrive {
    /// Physical impulse for an already drifted coordinate. The denominator
    /// accounts only for the drive's new velocity/position correction; free
    /// drift must not be predicted a second time.
    pub fn impulse(
        self,
        coordinate: f64,
        velocity: f64,
        inverse_mass: f64,
        dt: f64,
    ) -> Result<f64, &'static str> {
        if ![
            coordinate,
            velocity,
            self.target_position,
            self.target_velocity,
            inverse_mass,
            dt,
        ]
        .into_iter()
        .all(f64::is_finite)
            || inverse_mass < 0.0
            || dt < 0.0
            || self.max_force.is_nan()
            || self.max_force < 0.0
        {
            return Err(
                "coordinate drive requires finite state, nonnegative mobility/duration and a valid force cap",
            );
        }
        let (stiffness, damping, normalized) = match self.model {
            MotorModel::ForceBased { stiffness, damping } => (stiffness, damping, false),
            MotorModel::AccelerationBased { stiffness, damping } => (stiffness, damping, true),
            MotorModel::SpringDamper {
                frequency,
                damping_ratio,
            } => {
                if !frequency.is_finite()
                    || frequency < 0.0
                    || !damping_ratio.is_finite()
                    || damping_ratio < 0.0
                {
                    return Err(
                        "coordinate drive frequency and damping ratio must be finite and nonnegative",
                    );
                }
                let omega = std::f64::consts::TAU * frequency;
                (omega * omega, 2.0 * damping_ratio * omega, true)
            }
        };
        if ![stiffness, damping]
            .into_iter()
            .all(|value| value.is_finite() && value >= 0.0)
        {
            return Err("coordinate drive gains must be finite and nonnegative");
        }
        if dt == 0.0 || inverse_mass == 0.0 || self.max_force == 0.0 {
            return Ok(0.0);
        }
        let mobility = if normalized { 1.0 } else { inverse_mass };
        let denominator = 1.0 + mobility * dt * (damping + stiffness * dt);
        let effort = (stiffness * (self.target_position - coordinate)
            + damping * (self.target_velocity - velocity))
            / denominator;
        let force = if normalized {
            effort / inverse_mass
        } else {
            effort
        };
        if !denominator.is_finite() || denominator <= 0.0 || !force.is_finite() {
            return Err("coordinate drive response is not representable");
        }
        let impulse = force.clamp(-self.max_force, self.max_force) * dt;
        if !impulse.is_finite() {
            return Err("coordinate drive impulse is not representable");
        }
        Ok(impulse)
    }
}

struct RotationKernel;
impl AngularConstraint for RotationKernel {}

/// Apply one drive to a two-body joint or a multi-body relation. Jacobians
/// define the coordinate's rate and equal/opposite reaction distribution.
/// This writes only solver position increments. The shared velocity projection
/// realizes their velocity change exactly once after all position constraints.
pub fn integrate_coordinate_drive<const N: usize>(
    mut bodies: [&mut SolverBody; N],
    inertias: [&SolverBodyInertia; N],
    linear: [DVec3; N],
    angular: [DVec3; N],
    drive: CoordinateDrive,
    coordinate: f64,
    dt: f64,
) -> Result<f64, &'static str> {
    let least_dominance = inertias
        .iter()
        .map(|inertia| inertia.dominance())
        .min()
        .ok_or("coordinate drive requires at least one endpoint")?;
    let inertias: [_; N] = std::array::from_fn(|index| {
        if inertias[index].dominance() > least_dominance {
            &SolverBodyInertia::DUMMY
        } else {
            inertias[index]
        }
    });
    let mut inverse_mass = 0.0;
    let mut velocity = 0.0;
    for index in 0..N {
        let body = &bodies[index];
        let inertia = inertias[index];
        if ![
            linear[index],
            angular[index],
            body.linear_velocity,
            body.angular_velocity,
            body.delta_position,
            inertia.effective_inv_mass(),
        ]
        .into_iter()
        .all(DVec3::is_finite)
            || !inertia.effective_inv_mass().cmpge(DVec3::ZERO).all()
            || !inertia.effective_inv_angular_inertia().is_finite()
            || !crate::avian_backend_rotation_is_valid(body.delta_rotation.0)
        {
            return Err("coordinate drive requires finite solver frames, velocities and inertia");
        }
        inverse_mass += linear[index].dot(inertia.effective_inv_mass() * linear[index])
            + angular[index].dot(inertia.effective_inv_angular_inertia() * angular[index]);
        velocity +=
            linear[index].dot(body.linear_velocity) + angular[index].dot(body.angular_velocity);
    }
    let impulse = drive.impulse(coordinate, velocity, inverse_mass, dt)?;
    if impulse == 0.0 {
        return Ok(0.0);
    }
    let position_impulse = impulse * dt;
    let mut candidates: [SolverBody; N] = std::array::from_fn(|index| bodies[index].clone());
    for index in 0..N {
        let candidate = &mut candidates[index];
        let inertia = inertias[index];
        let rotation_vector =
            inertia.effective_inv_angular_inertia() * angular[index] * position_impulse;
        if !rotation_vector.is_finite() || rotation_vector.length() >= std::f64::consts::PI {
            return Err(
                "coordinate drive angular correction must be finite and below pi per substep",
            );
        }
        let rotation = <RotationKernel as AngularConstraint>::get_delta_rot(
            inertia.effective_inv_angular_inertia(),
            angular[index] * position_impulse,
        );
        candidate.delta_position += inertia.effective_inv_mass() * linear[index] * position_impulse;
        candidate.delta_rotation.0 = rotation * candidate.delta_rotation.0;
        if !candidate.delta_position.is_finite()
            || !crate::avian_backend_rotation_is_valid(candidate.delta_rotation.0)
        {
            return Err("coordinate drive position candidate is not a finite rigid state");
        }
    }
    for (body, candidate) in bodies.iter_mut().zip(candidates) {
        **body = candidate;
    }
    Ok(impulse)
}

#[cfg(test)]
mod tests {
    use super::*;

    use avian3d::{math::SymmetricMatrix, prelude::LockedAxes};

    fn inertia(locked: LockedAxes, dynamic: bool) -> SolverBodyInertia {
        SolverBodyInertia::new(1.0, SymmetricMatrix::IDENTITY, locked, 0, dynamic)
    }

    fn law() -> CoordinateDrive {
        CoordinateDrive {
            model: MotorModel::ForceBased {
                stiffness: 20.0,
                damping: 3.0,
            },
            target_position: 1.0,
            target_velocity: 0.0,
            max_force: f64::INFINITY,
        }
    }

    #[test]
    fn solver_update_respects_locked_axes_and_kinematic_authority() {
        let mut dynamic = SolverBody::DUMMY;
        let mut kinematic = SolverBody::DUMMY;
        let dynamic_inertia = inertia(LockedAxes::new().lock_translation_y(), true);
        let kinematic_inertia = inertia(LockedAxes::new(), false);
        let impulse = integrate_coordinate_drive(
            [&mut dynamic, &mut kinematic],
            [&dynamic_inertia, &kinematic_inertia],
            [-DVec3::X, DVec3::X],
            [DVec3::ZERO; 2],
            law(),
            0.0,
            0.01,
        )
        .unwrap();
        assert!(impulse > 0.0);
        assert!((dynamic.delta_position.x + impulse * 0.01).abs() < 1e-15);
        assert_eq!(kinematic.delta_position, DVec3::ZERO);
        assert_eq!(dynamic.linear_velocity, DVec3::ZERO);
        let before = dynamic.delta_position;
        assert_eq!(
            integrate_coordinate_drive(
                [&mut dynamic, &mut kinematic],
                [&dynamic_inertia, &kinematic_inertia],
                [-DVec3::Y, DVec3::Y],
                [DVec3::ZERO; 2],
                law(),
                0.0,
                0.01
            )
            .unwrap(),
            0.0
        );
        assert_eq!(dynamic.delta_position, before);
    }

    #[test]
    fn invalid_endpoint_correction_commits_no_partial_solver_state() {
        let mut first = SolverBody::DUMMY;
        let mut second = SolverBody::DUMMY;
        let mass = inertia(LockedAxes::new(), true);
        // The first endpoint has a representable translation; the second has
        // an angular increment beyond the admitted SO(3) projection interval.
        let fast = CoordinateDrive {
            target_position: 100.0,
            model: MotorModel::ForceBased {
                stiffness: 1000.0,
                damping: 0.0,
            },
            ..law()
        };
        assert!(
            integrate_coordinate_drive(
                [&mut first, &mut second],
                [&mass; 2],
                [DVec3::X, DVec3::ZERO],
                [DVec3::ZERO, DVec3::X],
                fast,
                0.0,
                1.0
            )
            .is_err()
        );
        assert_eq!(first.delta_position, DVec3::ZERO);
        assert_eq!(second.delta_rotation, SolverBody::DUMMY.delta_rotation);
    }

    #[test]
    fn stiff_implicit_law_remains_bounded_without_a_gain_cap() {
        let law = CoordinateDrive {
            model: MotorModel::ForceBased {
                stiffness: 400_000.0,
                damping: 0.0,
            },
            target_position: 0.0,
            ..law()
        };
        let (mut position, mut velocity) = (1.0, 0.0);
        let (inverse_mass, dt) = (1.0 / 60.0, 1.0 / 64.0);
        for _ in 0..2000 {
            position += dt * velocity;
            let impulse = law.impulse(position, velocity, inverse_mass, dt).unwrap();
            velocity += inverse_mass * impulse;
            position += dt * inverse_mass * impulse;
            assert!(position.is_finite() && velocity.is_finite());
        }
        assert!(position.abs() < 1e-12 && velocity.abs() < 1e-12);
    }

    #[test]
    fn post_drift_force_law_matches_implicit_position_and_damping() {
        for dt in [0.01, 0.02] {
            let law = CoordinateDrive {
                model: MotorModel::ForceBased {
                    stiffness: 20.0,
                    damping: 3.0,
                },
                target_position: 1.0,
                target_velocity: 0.4,
                max_force: f64::INFINITY,
            };
            let velocity = 0.3;
            let predicted = 0.2 + dt * velocity;
            let impulse = law.impulse(predicted, velocity, 0.7, dt).unwrap();
            let next_velocity = velocity + 0.7 * impulse;
            let next_position = predicted + dt * 0.7 * impulse;
            let force = 20.0 * (1.0 - next_position) + 3.0 * (0.4 - next_velocity);
            assert!((impulse - force * dt).abs() < 1e-14);
        }
    }

    #[test]
    fn caps_use_physical_units_for_every_model_and_mobility() {
        for model in [
            MotorModel::ForceBased {
                stiffness: 1000.0,
                damping: 10.0,
            },
            MotorModel::AccelerationBased {
                stiffness: 1000.0,
                damping: 10.0,
            },
            MotorModel::SpringDamper {
                frequency: 10.0,
                damping_ratio: 1.0,
            },
        ] {
            for inverse_mass in [0.01, 0.1, 1.0, 10.0] {
                for max_force in [0.0, 0.01] {
                    let law = CoordinateDrive {
                        model,
                        target_position: 1.0,
                        target_velocity: 0.0,
                        max_force,
                    };
                    assert_eq!(
                        law.impulse(0.0, 0.0, inverse_mass, 0.01).unwrap(),
                        max_force * 0.01
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_numeric_laws_fail_and_immovable_channels_do_not_move() {
        let law = CoordinateDrive {
            model: MotorModel::ForceBased {
                stiffness: 20.0,
                damping: 3.0,
            },
            target_position: 1.0,
            target_velocity: 0.0,
            max_force: f64::INFINITY,
        };
        assert_eq!(law.impulse(0.0, 0.0, 0.0, 0.01).unwrap(), 0.0);
        assert_eq!(law.impulse(0.0, 0.0, 1.0, 0.0).unwrap(), 0.0);
        assert!(law.impulse(f64::NAN, 0.0, 1.0, 0.01).is_err());
        assert!(law.impulse(0.0, 0.0, -1.0, 0.01).is_err());
        assert!(
            CoordinateDrive {
                max_force: -1.0,
                ..law
            }
            .impulse(0.0, 0.0, 1.0, 0.01)
            .is_err()
        );
        assert!(
            CoordinateDrive {
                model: MotorModel::ForceBased {
                    stiffness: -1.0,
                    damping: 0.0
                },
                ..law
            }
            .impulse(0.0, 0.0, 1.0, 0.01)
            .is_err()
        );
    }
}
