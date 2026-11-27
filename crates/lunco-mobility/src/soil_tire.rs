//! Rolling calibration and quasi-steady lateral soil resistance, evaluated at wheel load.
use crate::{
    TireLateralStiffnessGraph,
    soil_modelica::{SoilModelica, SoilModelicaCache},
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use lunco_physics::support::{ColliderTileOf, WheelSoil};

/// Native forces on a jointed wheel, averaged over the last master step.
#[derive(Component, Clone, Copy, Reflect, Default)]
#[reflect(Component)]
pub struct WheelForceSample {
    pub normal_n: f64,
    pub longitudinal_n: f64,
    pub lateral_n: f64,
    pub lateral_speed_mps: f64,
    pub slip_angle_rad: f64,
    pub sinkage_m: f64,
    pub drive_torque_nm: f64,
    pub rolling_moment_nm: f64,
    pub lateral_dissipation_w: f64,
}

#[derive(SystemParam)]
pub struct SoilSurfaces<'w, 's> {
    owners: Query<'w, 's, &'static ColliderTileOf>,
    materials: Query<'w, 's, &'static WheelSoil>,
    model: Option<NonSend<'w, SoilModelicaCache>>,
}

impl SoilSurfaces<'_, '_> {
    pub(crate) fn model(&self) -> Option<&SoilModelica> {
        self.model.as_ref().and_then(|model| model.get())
    }

    pub(crate) fn get(&self, collider: Entity) -> Option<&WheelSoil> {
        let owner = self.owners.get(collider).map_or(collider, |tile| tile.0);
        self.materials.get(owner).ok()
    }
}

#[derive(Clone, Default, Debug, Reflect)]
pub struct SoilSideForceCurve {
    pub knots: Vec<[f64; 2]>,
}
impl SoilSideForceCurve {
    pub fn new(knots: Vec<[f64; 2]>) -> Option<Self> {
        (knots.len() >= 2
            && knots[0] == [0.0, 0.0]
            && knots.iter().all(|p| {
                p[0].is_finite()
                    && p[0] <= std::f64::consts::FRAC_PI_2
                    && p[1].is_finite()
                    && p[1] >= 0.0
            })
            && knots.windows(2).all(|w| w[1][0] > w[0][0]))
        .then_some(Self { knots })
    }
    fn ratio(&self, angle: f64) -> f64 {
        for pair in self.knots.windows(2) {
            let [x0, y0] = pair[0];
            let [x1, y1] = pair[1];
            if angle <= x1 {
                return y0 + (angle - x0) / (x1 - x0) * (y1 - y0);
            }
        }
        self.knots.last().map_or(0.0, |p| p[1])
    }
}

/// One contact's fraction of a whole wheel's soil response. The parameters
/// are immutable across the implicit axle iterations; each query supplies only
/// the trial tread velocity.
pub(crate) struct PatchResponse<'a> {
    stiffness: f64,
    load: f64,
    soil: Option<SoilTrial<'a>>,
    pub sinkage_m: f64,
    pub rolling_moment_limit: f64,
    tread_gain_bound: f64,
}
struct SoilTrial<'a> {
    evaluator: Option<&'a SoilModelica>,
    inputs: [f64; 15],
}

impl<'a> PatchResponse<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        graph: TireLateralStiffnessGraph,
        curve: &SoilSideForceCurve,
        soil: Option<&WheelSoil>,
        load: f64,
        total: f64,
        radius: f64,
        width: f64,
        longitudinal_speed: f64,
        lateral_speed: f64,
        validated_speed: f64,
        mu: f64,
        evaluator: Option<&'a SoilModelica>,
    ) -> Self {
        let share = if total > 0.0 { load / total } else { 0.0 };
        let mut patch = Self {
            stiffness: share * graph.stiffness_at_load(total),
            load,
            soil: None,
            sinkage_m: 0.0,
            rolling_moment_limit: 0.0,
            tread_gain_bound: 0.0,
        };
        if let Some(soil) = soil.filter(|_| !curve.knots.is_empty() && load > 0.0) {
            let angle = lateral_speed.abs().atan2(longitudinal_speed.abs());
            let inputs = [
                total,
                load,
                radius,
                width,
                longitudinal_speed,
                lateral_speed,
                longitudinal_speed,
                soil.pressure_gradient,
                soil.cohesion,
                soil.friction_angle,
                soil.unit_weight,
                soil.shear_length,
                mu,
                curve.ratio(angle),
                validated_speed,
            ];
            let trial = SoilTrial { evaluator, inputs };
            let sample = trial.evaluate(longitudinal_speed);
            patch.sinkage_m = sample[2];
            patch.rolling_moment_limit = sample[5];
            patch.tread_gain_bound = sample[6];
            patch.soil = Some(trial);
        }
        patch
    }

    pub(crate) fn is_soil(&self) -> bool {
        self.soil.is_some()
    }

    /// Certify the torque evaluation at the representable-speed boundary.
    /// Both wheel realizations use the same contact law and roundoff bound;
    /// the housing term is zero for a raycast wheel. The stiffness argument
    /// is this contact's share of the authored longitudinal stiffness.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn axle_roundoff(
        &self,
        speed: f64,
        housing_speed: f64,
        radius: f64,
        v_long: f64,
        v_lat: f64,
        min_slip_speed: f64,
        min_validated_speed: f64,
        longitudinal_stiffness: f64,
        mu: f64,
    ) -> f64 {
        let tread = (speed + housing_speed) * radius;
        let velocity_scale = radius * (speed.abs() + housing_speed.abs()) + v_long.abs();
        if self.is_soil() {
            // Soil's geometric yaw demand is fixed during the axle solve;
            // its constitutive tread gain bounds the varying shear/moment.
            return 64.0
                * f64::EPSILON
                * (radius * self.tread_gain_bound * velocity_scale
                    + radius * mu * self.load
                    + self.rolling_moment_limit);
        }
        let reference = v_long.abs().max(tread.abs()).max(min_slip_speed);
        let rolling = v_long.abs().max(tread.abs()).max(min_validated_speed);
        let lateral_error = if self.stiffness == 0.0 || v_lat == 0.0 {
            0.0
        } else {
            let magnitude = rolling.hypot(v_lat);
            self.stiffness * (1.0 + (v_lat.abs() / magnitude) * (velocity_scale / magnitude))
        };
        // Near rolling, subtracting two nearly equal tread/hub speeds can
        // amplify a speed ulp by the unsaturated slip stiffness. Coulomb
        // projection is nonexpansive; include the lateral and cone scales.
        radius
            * (64.0 * f64::EPSILON)
            * (longitudinal_stiffness * velocity_scale / reference + lateral_error + mu * self.load)
    }

    #[cfg(test)]
    pub(crate) fn force(&self, tread: f64, long: f64, angle: f64, mu: f64) -> (f64, f64) {
        let (fx, fy, _) = self.wrench(tread, long, angle, mu);
        (fx, fy)
    }

    pub(crate) fn wrench(&self, tread: f64, long: f64, angle: f64, mu: f64) -> (f64, f64, f64) {
        if self.load <= 0.0 {
            return (0.0, 0.0, 0.0);
        }
        if let Some(soil) = &self.soil {
            let [fx, fy, _, _, moment, _, _] = soil.evaluate(tread);
            (fx, fy, moment)
        } else {
            let (fx, fy) = crate::limit_tire_shear(long, self.stiffness * angle, mu * self.load);
            (fx, fy, 0.0)
        }
    }
}

impl SoilTrial<'_> {
    fn evaluate(&self, tread: f64) -> [f64; 7] {
        let mut inputs = self.inputs;
        inputs[6] = tread;
        let Some(evaluator) = self.evaluator else {
            return [f64::NAN; 7];
        };
        match evaluator.evaluate(inputs) {
            Ok(result) => result,
            Err(error) => {
                bevy::log::error!("Modelica wheel reaction failed: {error}");
                [f64::NAN; 7]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rigid_axle_contact_roundoff_closes_stiff_near_rolling_cases() {
        let mut seed = 0xb876b87633a2dd50_u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 11) as f64 / (1_u64 << 53) as f64
        };
        let mu = 0.6;
        let dt = 1.0 / 60.0;
        let damping = 0.45;
        // Deterministic coverage of low loads, stiff near-rolling contacts,
        // and both signs of slip and shaft velocity. The rigid response is the
        // native contact law, not a mock rounding allowance.
        for _ in 0..100_000 {
            let radius = 0.1 + next() * 0.5;
            let inertia = 0.1 + next() * 7.0;
            let velocity = (next() * 2.0 - 1.0) * 3.0;
            let initial = velocity / radius + (next() * 2.0 - 1.0) * 0.01;
            let load = 10_f64.powf(next() * 6.0 - 3.0);
            let bound = radius * mu * load;
            let longitudinal_stiffness = 8_000.0 + next() * 12_000.0;
            let drive = (next() * 2.0 - 1.0) * bound;
            let response = PatchResponse::new(
                TireLateralStiffnessGraph::default(),
                &SoilSideForceCurve::default(),
                None,
                load,
                load,
                radius,
                0.28,
                velocity,
                0.0,
                1.07,
                mu,
                None,
            );
            let contact = |speed: f64| {
                let tread = speed * radius;
                let reference = tread.abs().max(velocity.abs()).max(0.1);
                response
                    .wrench(
                        tread,
                        longitudinal_stiffness * ((tread - velocity) / reference),
                        0.0,
                        mu,
                    )
                    .0
                    * radius
            };
            let roundoff = |speed| {
                response.axle_roundoff(
                    speed,
                    0.0,
                    radius,
                    velocity,
                    0.0,
                    0.1,
                    1.07,
                    longitudinal_stiffness,
                    mu,
                )
            };
            let result = crate::solve_axle_step_with_roundoff(
                initial, inertia, damping, drive, 0.0, bound, dt, contact, roundoff,
            )
            .unwrap_or_else(|error| {
                panic!("{error}: speed={initial:e}, v={velocity:e}, load={load:e}, drive={drive:e}")
            });
            let inertial = (inertia / dt + damping) * result.speed;
            let rhs = inertia / dt * initial + drive;
            let torque = contact(result.speed);
            let tolerance =
                (64.0 * f64::EPSILON * inertial.abs().max(torque.abs()).max(rhs.abs()).max(bound))
                    .max(roundoff(result.speed));
            assert!((inertial + torque - rhs).abs() <= tolerance);
        }
    }

    fn curve() -> SoilSideForceCurve {
        SoilSideForceCurve::new(vec![
            [0., 0.],
            [5f64.to_radians(), 0.24496],
            [10f64.to_radians(), 0.37195],
        ])
        .unwrap()
    }
    fn soil() -> WheelSoil {
        WheelSoil {
            pressure_gradient: 980000.,
            cohesion: 760.,
            friction_angle: 38.5f64.to_radians(),
            unit_weight: 2468.88,
            shear_length: 0.025,
        }
    }
    fn graph() -> TireLateralStiffnessGraph {
        TireLateralStiffnessGraph {
            rest_load: 182.25,
            minimum_normalized_load: 1.,
            max_stiffness: 371.,
        }
    }
    #[test]
    fn equivalent_tow_contact_subdivisions_preserve_force_and_moment() {
        let curve = curve();
        let model = SoilModelica::compile().unwrap();
        for material in [None, Some(soil())] {
            for speed in [0.1, 1.32] {
                for long_demand in [0., 500.] {
                    let resultant = |positions: &[f64]| {
                        let n = positions.len() as f64;
                        let mut sum = bevy::math::DVec3::ZERO;
                        let mut torque = sum;
                        for &x in positions {
                            let patch = PatchResponse::new(
                                graph(),
                                &curve,
                                material.as_ref(),
                                277. / n,
                                277.,
                                0.465,
                                0.28,
                                speed,
                                -0.115,
                                1.07,
                                0.6,
                                Some(&model),
                            );
                            let (fx, fy, rolling_moment) = patch.wrench(
                                speed + long_demand / 1000.,
                                long_demand / n,
                                5f64.to_radians(),
                                0.6,
                            );
                            let force = bevy::math::DVec3::new(fx, 0., fy);
                            sum += force;
                            torque += bevy::math::DVec3::new(x, -0.465, 0.).cross(force)
                                + bevy::math::DVec3::X * rolling_moment;
                        }
                        (sum, torque)
                    };
                    let (force, moment) = resultant(&[0.]);
                    for contacts in [&[-0.1, 0.1][..], &[-0.15, -0.05, 0.05, 0.15][..]] {
                        let (f, m) = resultant(contacts);
                        assert!((force - f).length() < 1e-10 && (moment - m).length() < 1e-10);
                    }
                }
            }
        }
    }
    #[test]
    fn rolling_curve_transfers_across_wes_load_groups() {
        let model = SoilModelica::compile().unwrap();
        let groups = [
            [(189., 41.5), (190., 63.7)],
            [(277., 71.4), (276., 113.)],
            [(365., 94.), (364., 135.1)],
        ];
        let mut absolute_error = 0.;
        for (held, group) in groups.iter().enumerate() {
            let mut knots = vec![[0., 0.]];
            for (i, degrees) in [5f64, 10.].iter().enumerate() {
                let ratio = groups
                    .iter()
                    .enumerate()
                    .filter(|(g, _)| *g != held)
                    .map(|(_, data)| data[i].1 / data[i].0)
                    .sum::<f64>()
                    / 2.;
                knots.push([degrees.to_radians(), ratio]);
            }
            let calibration = SoilSideForceCurve::new(knots).unwrap();
            for (i, &(load, measured)) in group.iter().enumerate() {
                let angle = (5. * (1 + i) as f64).to_radians();
                let p = PatchResponse::new(
                    graph(),
                    &calibration,
                    Some(&soil()),
                    load,
                    load,
                    0.465,
                    0.28,
                    1.32,
                    -1.32 * angle.tan(),
                    1.07,
                    0.6,
                    Some(&model),
                );
                let (_, lateral) = p.force(1.32, 0., angle, 0.6);
                let error = (lateral - measured).abs();
                assert!(error / measured < 0.18);
                absolute_error += error;
            }
        }
        assert!(absolute_error / 6. < 8.);
    }

    #[test]
    fn soil_contact_does_not_change_with_drive_mode_and_covers_high_yaw() {
        let model = SoilModelica::compile().unwrap();
        let curve = SoilSideForceCurve::new(
            [
                (0.0_f64, 0.0),
                (5., 0.24496),
                (10., 0.37195),
                (15., 0.481),
                (25., 0.517),
                (40., 0.533),
                (65., 0.809),
                (85., 1.266),
                (90., 1.107),
            ]
            .into_iter()
            .map(|(degrees, ratio)| [degrees.to_radians(), ratio])
            .collect(),
        )
        .unwrap();
        let patch = PatchResponse::new(
            graph(),
            &curve,
            Some(&soil()),
            182.25,
            182.25,
            0.465,
            0.28,
            0.2,
            0.1,
            1.07,
            0.6,
            Some(&model),
        );
        let passive = patch.force(0.21, 20.0, 0.1, 0.6);
        let active = patch.force(0.21, 381.0, 0.1, 0.6);
        assert_eq!(passive, active);
        eprintln!("identical kinematics: passive={passive:?} active={active:?}");
        for (degrees, load, speed, tread, observed) in [
            (40.0_f64, 274., 1.30, 0.8, 144.8),
            (65., 188., 1.31, 0.31, 152.2),
            (85., 186., 1.33, 0.10, 235.5),
            (90., 185., 1.31, 0.0, 204.9),
        ] {
            let angle = degrees.to_radians();
            let patch = PatchResponse::new(
                graph(),
                &curve,
                Some(&soil()),
                load,
                load,
                0.465,
                0.28,
                speed * angle.cos(),
                speed * angle.sin(),
                1.07,
                0.6,
                Some(&model),
            );
            let (fx, fy) = patch.force(tread, 0., angle, 0.6);
            assert!(fx.is_finite() && fy < 0.0);
            eprintln!(
                "WES {degrees} deg: load={load} measured={observed} lateral={} longitudinal={fx}",
                fy.abs()
            );
            assert!(
                (fy.abs() - observed).abs() / observed < 0.15,
                "{degrees} degrees: predicted {fy}, measured {observed}"
            );
        }
    }

    #[test]
    fn modelica_wrench_closes_implicit_axle_balance_and_compaction_work() {
        let model = SoilModelica::compile().unwrap();
        let patch = PatchResponse::new(
            graph(),
            &curve(),
            Some(&soil()),
            182.25,
            182.25,
            0.465,
            0.28,
            1.32,
            0.,
            1.07,
            0.6,
            Some(&model),
        );
        let radius = 0.465;
        let initial_speed = 1.32 / radius;
        let inertia = 1.0;
        let dt = 1.0 / 60.0;
        let reaction = |omega: f64| {
            let (fx, _, moment) = patch.wrench(omega * radius, 0., 0., 0.6);
            fx * radius + moment
        };
        let next = crate::solve_axle_step_with_roundoff(
            initial_speed,
            inertia,
            0.,
            0.,
            0.,
            radius * 0.6 * 182.25 + patch.rolling_moment_limit,
            dt,
            reaction,
            |omega| patch.axle_roundoff(omega, 0.0, radius, 1.32, 0.0, 0.1, 1.07, 0.0, 0.6),
        )
        .unwrap();
        let (fx, fy, moment) = patch.wrench(next.speed * radius, 0., 0., 0.6);
        assert!(next.speed < initial_speed && fx < 0. && fy == 0. && moment > 0.);
        assert!(moment * next.speed > 0.);
        assert!((inertia * (next.speed - initial_speed) / dt + reaction(next.speed)).abs() < 1e-8);
        let (_, _, reversed) = patch.wrench(-1.32, 0., 0., 0.6);
        assert!(reversed < 0.);
    }

    #[test]
    fn pivot_soil_resists_both_slip_signs_and_unloads_without_force() {
        let curve = curve();
        let model = SoilModelica::compile().unwrap();
        for slip in [-0.2_f64, 0., 0.2] {
            for load in [0., 182.25] {
                let patch = PatchResponse::new(
                    graph(),
                    &curve,
                    Some(&soil()),
                    load,
                    load,
                    0.465,
                    0.28,
                    0.,
                    slip,
                    1.07,
                    0.6,
                    Some(&model),
                );
                let (fx, fy) = patch.force(1., 1000., (-slip).atan2(1.07), 0.6);
                if load == 0. {
                    assert_eq!((fx, fy, patch.sinkage_m), (0., 0., 0.));
                } else {
                    assert!(patch.sinkage_m > 0. && fx <= 0.6 * load && fy * slip <= 0.);
                    assert_eq!(fy == 0., slip == 0.);
                }
            }
        }
        assert!(SoilSideForceCurve::new(vec![[0., 0.], [0., 0.2]]).is_none());
    }
}
