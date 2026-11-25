//! Rolling calibration and quasi-steady lateral soil resistance, evaluated at wheel load.
use crate::TireLateralStiffnessGraph;
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
    pub lateral_dissipation_w: f64,
}

#[derive(SystemParam)]
pub struct SoilSurfaces<'w, 's> {
    owners: Query<'w, 's, &'static ColliderTileOf>,
    materials: Query<'w, 's, &'static WheelSoil>,
}

impl SoilSurfaces<'_, '_> {
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

/// One physical patch's share of the whole wheel's lateral response.
/// Soil geometry and yield are independent of the implicit spin iterations.
#[derive(Clone, Copy)]
pub(crate) struct PatchResponse<'a> {
    stiffness: f64,
    curve: &'a SoilSideForceCurve,
    load: f64,
    rolling_weight: f64,
    shear_n: f64,
    passive_n: f64,
    pub sinkage_m: f64,
}

impl<'a> PatchResponse<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        graph: TireLateralStiffnessGraph,
        curve: &'a SoilSideForceCurve,
        soil: Option<&WheelSoil>,
        load: f64,
        total: f64,
        radius: f64,
        width: f64,
        longitudinal_speed: f64,
        lateral_speed: f64,
        validated_speed: f64,
    ) -> Self {
        let share = if total > 0.0 { load / total } else { 0.0 };
        let mut patch = Self {
            stiffness: share * graph.stiffness_at_load(total),
            curve,
            load,
            rolling_weight: 0.0,
            shear_n: 0.0,
            passive_n: 0.0,
            sinkage_m: 0.0,
        };
        if let Some(soil) = soil.filter(|_| !curve.knots.is_empty() && load > 0.0) {
            // Shallow cylindrical footprint L ~= sqrt(8 R z). Solve W = G z b L.
            let z = (total / (soil.pressure_gradient * width * (8.0 * radius).sqrt()))
                .powf(2.0 / 3.0)
                .min(radius);
            let length = (8.0 * radius * z).sqrt();
            let phi = soil.friction_angle;
            let root_kp = (std::f64::consts::FRAC_PI_4 + 0.5 * phi).tan();
            // Finite contact residence time regularizes quasi-static shear at rest.
            let displacement =
                lateral_speed.abs() * length / longitudinal_speed.abs().max(validated_speed);
            let mobilized = -(-displacement / soil.shear_length).exp_m1();
            patch.shear_n =
                share * (soil.cohesion * width * length + total * phi.tan()) * mobilized;
            // Integrated passive earth pressure over the leading side face.
            patch.passive_n = share
                * length
                * z
                * root_kp
                * (0.5 * soil.unit_weight * z * root_kp + 2.0 * soil.cohesion)
                * mobilized;
            patch.sinkage_m = z;
            let t = (longitudinal_speed.abs() / validated_speed).min(1.0);
            patch.rolling_weight = t * t * (3.0 - 2.0 * t);
        }
        patch
    }

    pub(crate) fn angular_gain_bound(&self) -> f64 {
        if self.sinkage_m == 0.0 {
            return self.stiffness;
        }
        let slope = self
            .curve
            .knots
            .windows(2)
            .map(|w| ((w[1][1] - w[0][1]) / (w[1][0] - w[0][0])).abs())
            .fold(0.0, f64::max);
        let [end, ratio] = *self.curve.knots.last().unwrap();
        let range = std::f64::consts::FRAC_PI_2 - end;
        self.load * slope
            + if range > 0.0 {
                (self.load * ratio + self.shear_n + self.passive_n) / range
            } else {
                0.0
            }
    }

    pub(crate) fn force(&self, long: f64, angle: f64, mu: f64) -> (f64, f64) {
        if self.load <= 0.0 {
            return (0.0, 0.0);
        }
        let (lateral, passive) = if self.sinkage_m > 0.0 {
            let magnitude = angle.abs();
            let end = self.curve.knots.last().unwrap()[0];
            let extrapolation = if magnitude > end {
                (magnitude - end) / (std::f64::consts::FRAC_PI_2 - end)
            } else {
                0.0
            };
            let rolling = self.rolling_weight * (1.0 - extrapolation);
            let low = 1.0 - rolling;
            (
                angle.signum()
                    * (rolling * self.load * self.curve.ratio(magnitude) + low * self.shear_n),
                angle.signum() * low * self.passive_n,
            )
        } else {
            (self.stiffness * angle, 0.0)
        };
        let (fx, fy) = crate::limit_tire_shear(long, lateral, mu * self.load);
        (fx, fy + passive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                            );
                            let (fx, fy) = patch.force(long_demand / n, 5f64.to_radians(), 0.6);
                            let force = bevy::math::DVec3::new(fx, 0., fy);
                            sum += force;
                            torque += bevy::math::DVec3::new(x, -0.465, 0.).cross(force);
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
                );
                let (_, lateral) = p.force(0., angle, 0.6);
                let error = (lateral - measured).abs();
                assert!(error / measured < 0.18);
                absolute_error += error;
            }
        }
        assert!(absolute_error / 6. < 8.);
    }

    #[test]
    fn pivot_soil_resists_both_slip_signs_and_unloads_without_force() {
        let curve = curve();
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
                );
                let (fx, fy) = patch.force(1000., (-slip).atan2(1.07), 0.6);
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
