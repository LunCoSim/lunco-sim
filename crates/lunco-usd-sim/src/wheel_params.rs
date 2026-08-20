//! # Unified wheel parameter model
//!
//! ONE reader for BOTH wheel kinds. A wheel prim's full dynamics — drivetrain
//! (peak torque, spin limits, brake), tire (μ, slip stiffness), inertia and
//! optional suspension compliance — are read here into a single [`WheelParams`],
//! regardless of whether the wheel is realised as a raycast wheel
//! (`lunco_mobility::WheelRaycast`, analytical suspension + contact) or a
//! physical wheel (Avian `RevoluteJoint` + normal-contact solver). Both
//! realizations call the same tire/drivetrain laws; only suspension and normal
//! contact acquisition differ. Every number they act on comes from the same
//! composed attributes with the same strictness.
//!
//! ## Attribute provenance
//!
//! PhysX-compatible names are used where NVIDIA's vehicle schema models the
//! concept; the runtime owns the realization-specific integration law — see
//! `core/physxSchema.usda`. `lunco:` names cover LunCo-only concepts:
//!
//! | Param | Attribute | Required |
//! |---|---|---|
//! | radius | `physxVehicleWheel:radius` | yes |
//! | width | `physxVehicleWheel:width` | yes |
//! | mass | `physxVehicleWheel:mass` | yes |
//! | moment of inertia | `physxVehicleWheel:moi` | yes (0 ⇒ derived ½·m·r² from authored mass+radius) |
//! | peak axle torque | MOTOR `lunco:motor:stallTorque` x gearbox `ratio` x `efficiency` | via motor |
//! | no-load axle speed | MOTOR `lunco:motor:noLoadSpeed` / gearbox `ratio` | via motor |
//! | bearing damping | `physxVehicleWheel:dampingRate` | yes |
//! | brake torque | `physxVehicleWheel:maxBrakeTorque` | yes |
//! | slip stiffness (longitudinal) | `physxVehicleTire:longitudinalStiffness` | yes |
//! | cornering stiffness, N/rad | `physxVehicleTire:lateralStiffness` | no (schema fallback 0.0) |
//! | Coulomb μ | `physics:dynamicFriction` (`UsdPhysicsMaterialAPI`) | yes |
//! | steer axis | `lunco:wheel:steerAxis` | yes |
//! | motor damping | `lunco:wheel:driveDamping` | yes |
//! | suspension | `lunco:suspension:restLength` + `physxVehicleSuspension:springStrength`/`:springDamperRate` | raycast only |
//!
//! ## One no-load speed for both realizations
//!
//! `lunco:motor:noLoadSpeed` reduced by `lunco:gearbox:ratio` is THE no-load axle speed, and both
//! kinds obey it: the joint wheel's velocity motor targets it
//! (`MotorActuator::max_omega`), and the raycast wheel rolls its shared tire
//! drive solve off toward it, so both self-limit at
//! `ω_max · r`. The former wheel-local speed names are gone; there is no alias
//! and no fallback.
//!
//! ## Strictness
//!
//! NO Rust fallback values. Every required attribute missing from the composed
//! prim is an asset error, collected so one bad wheel reports ALL of them, not
//! just the first. The authored defaults live in
//! `components/mobility/wheel.usda`, which every wheel composes — one authored
//! set is what makes "same defaults for both variants" true.

use avian3d::prelude::{
    AngularMotor, Collider, ColliderDensity, Friction, MotorModel, Position, RevoluteJoint,
    Rotation,
};
use bevy::asset::AssetId;
use bevy::log::{info, warn};
use bevy::math::DVec3;
use bevy::prelude::{Entity, Quat, World};
use lunco_hardware::{MotorActuator, SteeringActuator};
use lunco_mobility::{JointedWheelTire, Suspension, WheelRaycast};
use lunco_usd_bevy::{CanonicalStages, UsdPrimPath, UsdRead, UsdStageAsset};
use openusd::sdf::Path as SdfPath;
use std::collections::HashMap;

/// Authored suspension compliance, shared by both wheel implementations. The
/// raycast wheel emulates this spring analytically; a joint wheel is a rigid
/// axle and does not need it.
///
/// `spring_k` / `damping_c` come from NVIDIA's canonical
/// `PhysxVehicleSuspensionAPI` names (`physxVehicleSuspension:springStrength` /
/// `:springDamperRate`). `rest_length` has no PhysX equivalent — PhysX models
/// travel as `travelDistance` + `sprungMass` — so it is authored as
/// `lunco:suspension:restLength`.
#[derive(Clone, Copy, Debug)]
pub struct SuspensionParams {
    /// Natural standoff of the wheel below its mount (raycast resting length), m.
    pub rest_length: f64,
    /// Spring stiffness, N/m.
    pub spring_k: f64,
    /// Spring damping, N·s/m.
    pub damping_c: f64,
}

/// The complete authored dynamics of one wheel — the single source both
/// `setup_raycast_wheel` and `setup_physical_wheel` consume, and the single
/// struct the live resync path re-derives.
#[derive(Clone, Copy, Debug)]
pub struct WheelParams {
    /// Wheel radius, m (`physxVehicleWheel:radius`).
    pub radius: f64,
    /// Wheel width along its authored cylinder axis, m (`physxVehicleWheel:width`).
    /// This standard wheel value drives the collider in both realizations.
    pub width: f64,
    /// Authored cylinder/axle axis (`axis` token).  Avian's primitive cylinder
    /// uses local +Y, so the physical projection rotates that primitive onto this
    /// authored axis and the revolute joint uses the same vector.
    pub axle_axis: DVec3,
    /// Wheel mass, kg (`physxVehicleWheel:mass`). The same authored value feeds both
    /// realizations; any difference in feel must come from the solver, not a Rust fork.
    pub mass: f64,
    /// Explicit axle moment of inertia, kg·m² (`physxVehicleWheel:moi`).
    /// An authored zero means the documented solid-cylinder derivation
    /// `½·m·r²` from the authored mass and radius; the attribute itself is still
    /// required by the standard PhysX wheel schema.
    pub moment_of_inertia: f64,
    /// Rotor inertia reflected through the gearbox to the axle, kg·m²
    /// (`J·ratio²`, from the motor behind this wheel). `0` for an undriven
    /// wheel. At the shipped reductions it dominates the tire's ½·m·r² —
    /// see [`crate::powertrain::PowertrainParams::reflected_inertia`].
    pub reflected_inertia: f64,
    /// Peak axle drive torque, N·m, reduced from the composed motor and gearbox.
    pub peak_torque: f64,
    /// No-load axle speed, rad/s, reduced from the composed motor and gearbox.
    /// This is the top-speed parameter for BOTH realizations: the joint motor
    /// targets it, the raycast drive force rolls off toward it, so both cap at
    /// `ω·r`.
    pub max_rotation_speed: f64,
    /// Bearing + rolling drag, N·m·s (`physxVehicleWheel:dampingRate`). A
    /// physical property of the hub in its own right — REQUIRED, never inferred
    /// from the drive torque.
    pub bearing_damping: f64,
    /// Lock-up authority, N·m (`physxVehicleWheel:maxBrakeTorque`).
    pub brake_torque_max: f64,
    /// Tire longitudinal stiffness (`physxVehicleTire:longitudinalStiffness`).
    pub slip_stiffness: f64,
    /// Tire CORNERING stiffness (`physxVehicleTire:lateralStiffness`) — side
    /// force per RADIAN of slip angle, before the Coulomb cone. This is
    /// consumed by the shared tire model for both raycast and jointed wheels.
    /// Avian supplies only a jointed wheel's normal contact solve.
    ///
    /// Read on the schema's own terms: "cornering stiffness" means N/rad in PhysX
    /// and in vehicle-dynamics texts. The parity scene checks that both
    /// realizations turn with the same authored contact law and vehicle/motor
    /// contract.
    ///
    /// The PhysX schema's own companion to `longitudinalStiffness`, and read on
    /// the schema's terms: it declares a `0.0` fallback, so an unauthored tire
    /// resolves to zero here rather than raising a missing-attribute error. Zero
    /// is a legal (if unhelpful) tire — no cornering grip at all — and the place
    /// to state a real value is the tire asset, which every shipped tire does.
    pub cornering_stiffness: f64,
    /// Lower edge of the tire's measured steady-state cornering-speed envelope.
    pub min_validated_speed: f64,
    /// Coulomb μ from the wheel's standard `UsdPhysicsMaterialAPI`, composed
    /// through the `tire` variant. Both realizations use it as the shared tire
    /// cone; Avian's generic tangent friction is disabled for jointed tire
    /// contacts so it cannot double-count this force.
    pub friction_mu: f64,
    /// Raked steering-head axis, wheel-local (`lunco:wheel:steerAxis`).
    pub steer_axis: DVec3,
    /// Velocity-tracking aggressiveness, 1/s (`lunco:wheel:driveDamping`).
    pub drive_damping: f64,
    /// Suspension compliance; `None` ⇒ none resolves. A raycast wheel treats
    /// that as a hard asset error, a joint wheel does not need it.
    pub suspension: Option<SuspensionParams>,
}

impl WheelParams {
    /// Read every wheel attribute off the composed prim, collecting ALL missing
    /// required names into the error. `attachment_suspension` is the suspension
    /// prim selected by the standard `PhysxVehicleWheelAttachmentAPI`; direct
    /// wheel/suspension API composition passes the wheel itself. A wheel without
    /// an attachment is under-authored and is rejected.
    ///
    /// `powertrain` is the motor (and optional gearbox) that turns this wheel, found
    /// by the caller via `lunco:motor:drivenWheel`. Torque and no-load speed come from
    /// it, NOT from the wheel. `None` means an undriven wheel (a castor, a trailer wheel):
    /// zero torque, and legitimate to author.
    pub fn read(
        reader: &lunco_usd_bevy::StageView<'_>,
        wheel: &SdfPath,
        attachment_suspension: Option<&SdfPath>,
        powertrain: Option<&crate::powertrain::PowertrainParams>,
    ) -> Result<WheelParams, Vec<String>> {
        let mut missing = Vec::new();
        let axle_axis = match reader.text(wheel, "axis").as_deref() {
            Some("X") => DVec3::X,
            Some("Y") => DVec3::Y,
            Some("Z") => DVec3::Z,
            _ => {
                missing.push("axis".to_owned());
                DVec3::X
            }
        };
        let mut req = |name: &'static str| -> f64 {
            match reader.real(wheel, name) {
                Some(v) => v,
                None => {
                    missing.push(name.to_owned());
                    0.0
                }
            }
        };

        let radius = req("physxVehicleWheel:radius");
        let width = req("physxVehicleWheel:width");
        let mass = req("physxVehicleWheel:mass");
        // From the MOTOR behind the wheel, geared. An undriven wheel has no motor and
        // therefore no torque — that is a castor, not a wheel with a default torque.
        let peak_torque = powertrain.map_or(0.0, |p| p.axle_peak_torque());
        // An undriven wheel has no motor speed cap. A driven wheel's motor
        // contract rejects a non-positive no-load speed before it reaches this
        // reader, so zero here is an honest castor value rather than a numeric
        // substitute for a malformed powertrain.
        let max_rotation_speed = powertrain.map_or(0.0, |p| p.axle_no_load_speed());
        let reflected_inertia = powertrain.map_or(0.0, |p| p.reflected_inertia());
        let bearing_damping = req("physxVehicleWheel:dampingRate");
        let brake_torque_max = req("physxVehicleWheel:maxBrakeTorque");
        let slip_stiffness = req("physxVehicleTire:longitudinalStiffness");
        // NOT `req`: the PhysX schema declares this one with a `0.0` fallback, so
        // it always composes to a value and there is nothing to report missing.
        // Reading it any other way would invent a required-ness the schema does
        // not state — see the field doc.
        let cornering_stiffness = reader
            .real(wheel, "physxVehicleTire:lateralStiffness")
            .unwrap_or(0.0);
        let min_validated_speed = reader
            .real(wheel, "lunco:tire:minValidatedSpeed")
            .unwrap_or(0.0);
        let friction_mu = req("physics:dynamicFriction");
        let drive_damping = req("lunco:wheel:driveDamping");

        // A zero is an authored, documented solid-cylinder derivation. The
        // standard PhysX wheel attribute itself remains required, so an
        // omitted value is reported with the other missing contract fields.
        let moment_of_inertia = req("physxVehicleWheel:moi");

        let steer_axis = match lunco_usd_bevy::read_vec3_f64(reader, wheel, "lunco:wheel:steerAxis")
        {
            Some(v) => DVec3::new(v[0], v[1], v[2]),
            None => {
                missing.push("lunco:wheel:steerAxis".to_owned());
                DVec3::Y
            }
        };

        if !missing.is_empty() {
            return Err(missing);
        }

        validate_wheel_values(
            &mut missing,
            radius,
            width,
            mass,
            moment_of_inertia,
            bearing_damping,
            brake_torque_max,
            slip_stiffness,
            cornering_stiffness,
            min_validated_speed,
            friction_mu,
            steer_axis,
            drive_damping,
        );
        if !missing.is_empty() {
            return Err(missing);
        }

        // The shipped compact composition applies the attachment, wheel, and
        // suspension APIs to one composed prim. That is an explicit standard
        // direct-composition form, not a heuristic fallback. Relationship-form
        // assets pass the referenced suspension from the stage topology map.
        let direct_suspension = (attachment_suspension.is_none()
            && reader.has_api_schema(wheel, "PhysxVehicleWheelAttachmentAPI")
            && reader.has_api_schema(wheel, "PhysxVehicleSuspensionAPI"))
        .then_some(wheel);
        let suspension_prim = attachment_suspension.or(direct_suspension);
        let suspension = match suspension_prim {
            Some(susp) => match read_suspension_attrs(reader, susp) {
                Ok(params) => Some(params),
                Err(errors) => {
                    missing.extend(errors);
                    None
                }
            },
            None => {
                missing.push("PhysxVehicleWheelAttachmentAPI".to_owned());
                None
            }
        };

        if !missing.is_empty() {
            return Err(missing);
        }

        Ok(WheelParams {
            radius,
            width,
            axle_axis,
            mass,
            moment_of_inertia,
            reflected_inertia,
            peak_torque,
            max_rotation_speed,
            bearing_damping,
            brake_torque_max,
            slip_stiffness,
            cornering_stiffness,
            min_validated_speed,
            friction_mu,
            steer_axis,
            drive_damping,
            suspension,
        })
    }

    /// The raycast realisation: a `WheelRaycast` carrying these numbers.
    pub fn to_wheel_raycast(
        &self,
        drive_port: Entity,
        steer_port: Entity,
        visual_entity: Option<Entity>,
    ) -> WheelRaycast {
        let mut wheel = WheelRaycast {
            wheel_radius: self.radius,
            visual_entity,
            drive_port,
            steer_port,
            ..Default::default()
        };
        self.apply_to_raycast(&mut wheel);
        wheel
    }

    /// Write the tunable numbers into an existing `WheelRaycast` — the same
    /// mapping `to_wheel_raycast` uses, exposed so the live resync path can
    /// re-derive a spawned wheel in place (ports/visual/state untouched).
    pub fn apply_to_raycast(&self, wheel: &mut WheelRaycast) {
        wheel.wheel_radius = self.radius;
        wheel.mass = self.mass;
        wheel.moment_of_inertia = self.moment_of_inertia;
        wheel.reflected_inertia = self.reflected_inertia;
        wheel.drive_torque_max = self.peak_torque;
        wheel.max_rotation_speed = self.max_rotation_speed;
        wheel.bearing_damping = self.bearing_damping;
        wheel.friction_mu = self.friction_mu;
        wheel.slip_stiffness = self.slip_stiffness;
        wheel.cornering_stiffness = self.cornering_stiffness;
        wheel.min_validated_speed = self.min_validated_speed;
        wheel.brake_torque_max = self.brake_torque_max;
        wheel.steer_axis = self.steer_axis;
    }

    /// Write the suspension compliance into an existing `Suspension`.
    /// Returns `false` (untouched) when this wheel resolves no suspension.
    pub fn apply_to_suspension(&self, suspension: &mut Suspension) -> bool {
        let Some(susp) = self.suspension else {
            return false;
        };
        suspension.rest_length = susp.rest_length;
        suspension.spring_k = susp.spring_k;
        suspension.damping_c = susp.damping_c;
        true
    }

    /// The physical wheel's axle motor model. The
    /// [`lunco_hardware::MotorActuator`] enables it only for a non-zero command
    /// and writes the live motor/gearbox torque curve every tick.
    ///
    /// The torque cap is deliberately NOT fixed here. It is the authored motor
    /// curve `τ(ω) = τ_stall·(1 − ω/ω_noload)`, which depends on this tick's axle
    /// rate, so the actuator writes `motor.max_torque` every tick; a constant cap
    /// authored at build time would turn the physical wheel into a speed source
    /// disconnected from its motor power.
    ///
    /// It is built DISABLED, not left at `AngularMotor::new`'s `Scalar::MAX`
    /// torque. A motor born with unbounded torque and a target velocity of zero is
    /// an infinitely strong brake, and the physics steps between the joint being
    /// spawned and the actuator's first write were enough to fire an unbounded
    /// impulse and throw every body in the rig out of the world
    /// (`[physics] body left the world`, velocities of ~1e6 m/s). Note that
    /// `with_max_torque(0.0)` does NOT express "no torque" in avian — zero is a
    /// sentinel for UNLIMITED, see the warning in `MotorActuator`. Disabled is the
    /// honest starting point in any case: an axle exerts no torque until something
    /// commands it, and the actuator enables the motor on the first commanded tick.
    ///
    /// `lunco:wheel:driveDamping` is what remains meaningful: with the torque
    /// capped by the curve the motor is saturated for nearly the whole speed
    /// range, so the damping only shapes the last approach to no-load, where the
    /// curve has already fallen to near zero. That is why sweeping it 30 → 150
    /// produced byte-identical traces — not a broken parameter, a parameter whose
    /// regime the rover never entered.
    pub fn drive_motor(&self) -> AngularMotor {
        AngularMotor::new_disabled(MotorModel::AccelerationBased {
            stiffness: 0.0,
            damping: self.drive_damping,
        })
        // `0` is Avian's unlimited sentinel, but the motor is disabled until
        // MotorActuator writes a positive live torque cap in the first command.
        .with_max_torque(0.0)
    }

    /// Axle moment of inertia, kg·m² — authored `physxVehicleWheel:moi` when it
    /// is stated, otherwise the solid-disk derivation `½·m·r²` from the authored
    /// mass and radius — plus the drivetrain's reflected rotor inertia.
    ///
    /// The SAME derivation `WheelRaycast::axle_inertia` applies on the raycast
    /// side (it cannot be shared as code — `lunco-mobility` does not depend on
    /// this crate — so it is shared as a rule, and both are fed by this reader).
    /// The physical wheel's tire term comes from its cylinder collider at
    /// `wheel_density()`, which is ½·m·r² about the axle by construction; the
    /// reflected term is stamped on top as an explicit `AngularInertia`
    /// override at spawn, so both realizations integrate the same total.
    pub fn axle_inertia(&self) -> f64 {
        let tire = if self.moment_of_inertia > 0.0 {
            self.moment_of_inertia
        } else {
            0.5 * self.mass * self.radius * self.radius
        };
        tire + self.reflected_inertia
    }

    /// Collider density realising `physxVehicleWheel:mass` on the physical wheel's
    /// cylinder collider (`cylinder(r, h = physxVehicleWheel:width)` ⇒ volume
    /// = π·r²·width).
    ///
    /// Mass goes in via DENSITY, not a forced `Mass`: avian derives
    /// `AngularInertia` from the collider at `ColliderDensity` even when `Mass`
    /// is set, and a forced mass desyncs mass from angular inertia — the
    /// contact+joint solver then can't build enough support impulse and the
    /// rover sinks through the one-sided terrain heightfield.
    pub fn wheel_density(&self) -> f32 {
        let volume = std::f64::consts::PI * self.radius.powi(2) * self.width;
        (self.mass / volume.max(1e-6)) as f32
    }
}

/// Resolve a wheel's attachment suspension prim via the standard attachment
/// topology. The map belongs to one composed stage, so its keys are stage-local
/// paths; independent instances retain independent topology maps.
pub(crate) fn attachment_suspension_path(
    wheel_path: &str,
    wheel_attachment_targets: &HashMap<String, String>,
) -> Option<SdfPath> {
    wheel_attachment_targets
        .get(wheel_path)
        .and_then(|s| SdfPath::new(s).ok())
}

/// Read the three suspension attrs off one prim. `None` unless all three are
/// authored — partial authoring is treated as missing (no per-field defaults).
fn read_suspension_attrs(
    reader: &lunco_usd_bevy::StageView<'_>,
    prim: &SdfPath,
) -> Result<SuspensionParams, Vec<String>> {
    let mut missing = Vec::new();
    let read = |name: &str, missing: &mut Vec<String>| {
        reader.real(prim, name).or_else(|| {
            missing.push(name.to_owned());
            None
        })
    };
    let rest_length = read("lunco:suspension:restLength", &mut missing);
    let spring_k = read("physxVehicleSuspension:springStrength", &mut missing);
    let damping_c = read("physxVehicleSuspension:springDamperRate", &mut missing);
    if !missing.is_empty() {
        return Err(missing);
    }

    let (Some(rest_length), Some(spring_k), Some(damping_c)) = (rest_length, spring_k, damping_c)
    else {
        unreachable!("missing suspension values were rejected above")
    };
    let mut invalid = Vec::new();
    validate_suspension_values(&mut invalid, rest_length, spring_k, damping_c);
    if !invalid.is_empty() {
        return Err(invalid);
    }

    Ok(SuspensionParams {
        rest_length,
        spring_k,
        damping_c,
    })
}

fn validate_suspension_values(
    errors: &mut Vec<String>,
    rest_length: f64,
    spring_k: f64,
    damping_c: f64,
) {
    validate_range(errors, "lunco:suspension:restLength", rest_length, 0.0, 2.0);
    validate_range(
        errors,
        "physxVehicleSuspension:springStrength",
        spring_k,
        0.0,
        100_000.0,
    );
    validate_range(
        errors,
        "physxVehicleSuspension:springDamperRate",
        damping_c,
        0.0,
        20_000.0,
    );
}

fn validate_wheel_values(
    errors: &mut Vec<String>,
    radius: f64,
    width: f64,
    mass: f64,
    moment_of_inertia: f64,
    bearing_damping: f64,
    brake_torque_max: f64,
    slip_stiffness: f64,
    cornering_stiffness: f64,
    min_validated_speed: f64,
    friction_mu: f64,
    steer_axis: DVec3,
    drive_damping: f64,
) {
    validate_range(errors, "physxVehicleWheel:radius", radius, 0.05, 3.0);
    validate_positive(errors, "physxVehicleWheel:width", width);
    validate_positive(errors, "physxVehicleWheel:mass", mass);
    validate_range(
        errors,
        "physxVehicleWheel:moi",
        moment_of_inertia,
        0.0,
        50.0,
    );
    validate_range(
        errors,
        "physxVehicleWheel:dampingRate",
        bearing_damping,
        0.0,
        50.0,
    );
    validate_range(
        errors,
        "physxVehicleWheel:maxBrakeTorque",
        brake_torque_max,
        0.0,
        5_000.0,
    );
    validate_range(
        errors,
        "physxVehicleTire:longitudinalStiffness",
        slip_stiffness,
        0.0,
        30_000.0,
    );
    validate_nonnegative(
        errors,
        "physxVehicleTire:lateralStiffness",
        cornering_stiffness,
    );
    validate_range(
        errors,
        "lunco:tire:minValidatedSpeed",
        min_validated_speed,
        0.0,
        10.0,
    );
    validate_nonnegative(errors, "physics:dynamicFriction", friction_mu);
    validate_nonnegative(errors, "lunco:wheel:driveDamping", drive_damping);
    if !(steer_axis.is_finite() && steer_axis.length_squared() > 0.0) {
        errors.push(format!(
            "lunco:wheel:steerAxis must be finite and non-zero, got {steer_axis:?}"
        ));
    }
}

fn validate_positive(errors: &mut Vec<String>, name: &str, value: f64) {
    if !(value.is_finite() && value > 0.0) {
        errors.push(format!("{name} must be finite and > 0, got {value}"));
    }
}

fn validate_nonnegative(errors: &mut Vec<String>, name: &str, value: f64) {
    if !(value.is_finite() && value >= 0.0) {
        errors.push(format!("{name} must be finite and >= 0, got {value}"));
    }
}

fn validate_range(errors: &mut Vec<String>, name: &str, value: f64, min: f64, max: f64) {
    if !(value.is_finite() && (min..=max).contains(&value)) {
        errors.push(format!(
            "{name} must be finite and in [{min}, {max}], got {value}"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_suspension_values, validate_wheel_values};
    use bevy::math::DVec3;

    #[test]
    fn authored_wheel_values_accept_the_documented_contract() {
        let mut errors = Vec::new();
        validate_wheel_values(
            &mut errors,
            0.3,
            0.2,
            12.0,
            0.54,
            0.5,
            120.0,
            14_000.0,
            8_000.0,
            0.0,
            1.5,
            DVec3::Y,
            30.0,
        );
        assert!(
            errors.is_empty(),
            "unexpected validation errors: {errors:?}"
        );
    }

    #[test]
    fn authored_wheel_values_reject_nonfinite_and_out_of_contract_numbers() {
        let mut errors = Vec::new();
        validate_wheel_values(
            &mut errors,
            f64::NAN,
            0.0,
            f64::INFINITY,
            51.0,
            -1.0,
            5_001.0,
            30_001.0,
            -1.0,
            11.0,
            -0.1,
            DVec3::ZERO,
            -1.0,
        );
        for name in [
            "physxVehicleWheel:radius",
            "physxVehicleWheel:width",
            "physxVehicleWheel:mass",
            "physxVehicleWheel:moi",
            "physxVehicleWheel:dampingRate",
            "physxVehicleWheel:maxBrakeTorque",
            "physxVehicleTire:longitudinalStiffness",
            "physxVehicleTire:lateralStiffness",
            "lunco:tire:minValidatedSpeed",
            "physics:dynamicFriction",
            "lunco:wheel:driveDamping",
            "lunco:wheel:steerAxis",
        ] {
            assert!(
                errors.iter().any(|error| error.starts_with(name)),
                "missing validation error for {name}: {errors:?}"
            );
        }
    }

    #[test]
    fn authored_suspension_values_reject_nonfinite_and_out_of_contract_numbers() {
        let mut errors = Vec::new();
        validate_suspension_values(&mut errors, -0.01, f64::NAN, 20_001.0);
        assert_eq!(errors.len(), 3, "unexpected validation errors: {errors:?}");
        assert!(errors[0].starts_with("lunco:suspension:restLength"));
        assert!(errors[1].starts_with("physxVehicleSuspension:springStrength"));
        assert!(errors[2].starts_with("physxVehicleSuspension:springDamperRate"));
    }

    #[test]
    fn zero_suspension_rest_length_is_valid_only_as_an_authored_rigid_mount() {
        let mut errors = Vec::new();
        validate_suspension_values(&mut errors, 0.0, 15_000.0, 5_000.0);
        assert!(
            errors.is_empty(),
            "unexpected validation errors: {errors:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Live resync — the USD-based update path for spawned wheels.
//
// Wheel params are a PROJECTION of the document: the only writer is the USD
// document itself (`ApplyUsdOp SetAttribute` → registry → the change funnels in
// `twin_projection`/`live_consume`), and this module is how the projection
// catches up — by RE-READING the composed stage, never by accepting values from
// a side channel. Both funnels call [`resync_wheels_for_stage`] for edits that
// [`claims_edit`] recognises, INSTEAD of their generic
// `refresh_prim_subtree`/`reinstantiate_entity` fallback. That fallback is
// actively destructive for wheels: it despawns the wheel's synthesized
// `Port` children and visual child while `UsdSimProcessed` survives, so
// the sim params are never re-derived, the `MotorActuator` points at a dead
// port, and the chassis-owned joint dangles. The resync mutates the spawned
// components in place — entity ids, joints, `JointCollisionDisabled`, ports and
// `UsdSimProcessed` are never touched.
// ---------------------------------------------------------------------------

/// Attribute families [`resync_wheels_for_stage`] claims from the generic
/// refresh fallback. Prim-scoped where a name is not wheel-specific:
/// `physxVehicleWheel:mass` is claimed only on a wheel prim — on a chassis it must keep
/// the normal refresh path (mass overrides are rebuilt by `lunco-usd-avian`).
pub fn claims_edit(reader: &lunco_usd_bevy::StageView<'_>, prim: &SdfPath, attr: &str) -> bool {
    if attr.starts_with("physxVehicleWheel:") {
        return reader.has_api_schema(prim, "PhysxVehicleWheelAPI");
    }
    const WHEEL_ONLY_PREFIXES: [&str; 5] = [
        "lunco:wheel:",
        "lunco:suspension:",
        "lunco:tire:",
        "physxVehicleTire:",
        "physxVehicleSuspension:",
    ];
    if WHEEL_ONLY_PREFIXES.iter().any(|p| attr.starts_with(p)) {
        return true;
    }
    // Torque and speed belong to the composed motor/gearbox parts.  A live edit
    // on either part must re-read every wheel that consumes that powertrain, but
    // an identically named attribute on an unrelated prim must keep the normal
    // document refresh path.
    if attr.starts_with("lunco:motor:") || attr.starts_with("lunco:gearbox:") {
        return reader.has_api_schema(prim, "LunCoMotorAPI")
            || reader.has_api_schema(prim, "LunCoGearboxAPI");
    }
    // Vehicle-root knobs: steering lock and drive-kernel selection re-derive in
    // place; a subtree refresh of the whole rover root would tear down live
    // physics bodies.
    if attr == "physxVehicleAckermannSteering:maxSteerAngle"
        || attr == "physxVehicleAckermannSteering:strength"
        || attr == "lunco:driveKernel"
    {
        return true;
    }
    // A connection transform on a `DriveMix` term prim (`lunco:factor:throttle`
    // and friends). `resync_wheels_for_stage` re-derives EVERY vehicle root of
    // the stage, so claiming the edit on the term prim resyncs the mix it
    // belongs to without the caller resolving the owning vessel. The prefix is
    // shared with the co-simulation port graph, so the claim is scoped to prims
    // under a `DriveMix` scope — a factor on a cosim connection is not a wheel
    // edit and must keep the normal refresh path.
    if attr.starts_with("lunco:factor:") {
        return prim
            .as_str()
            .rsplit_once('/')
            .and_then(|(parent, _)| parent.rsplit_once('/'))
            .is_some_and(|(_, scope)| scope == "DriveMix");
    }
    false
}

/// One wheel's re-read result, staged so the `!Send` stage borrow is released
/// before the world is mutated.
struct WheelUpdate {
    entity: Entity,
    physical: bool,
    params: WheelParams,
    /// Steering lock from the wheel's vehicle, when it has a steering system.
    max_steer_angle: Option<f64>,
    /// Ackermann correction strength from the owning vehicle.
    ackermann_strength: f64,
}

/// Re-derive every spawned wheel (and vehicle-root drive mix) of `stage` from
/// the live composed stage, IN PLACE. Resyncs ALL wheels of the stage rather
/// than only the edited prim: suspension/tire attrs may be authored on a
/// separate referenced prim (attachment topology), vehicle-level attrs fan out
/// to every wheel, and a rover has ≤6 wheels — re-reading them all is cheap and
/// makes the resync a fixed point (double-firing from both funnels is
/// harmless).
///
/// A wheel whose re-read now FAILS (a half-authored edit removed a required
/// attr) keeps its old values — never break a running wheel; the collected
/// missing-attr warning names what to restore.
pub fn resync_wheels_for_stage(world: &mut World, id: AssetId<UsdStageAsset>) {
    // 1. Collect this stage's spawned wheels + vehicle roots (plain data out).
    let mut rows: Vec<(Entity, String, bool)> = Vec::new();
    {
        let mut q = world.query::<(
            Entity,
            &UsdPrimPath,
            Option<&WheelRaycast>,
            Option<&crate::PhysicalWheel>,
        )>();
        for (e, prim, rc, pw) in q.iter(world) {
            if prim.stage_handle.id() != id || (rc.is_none() && pw.is_none()) {
                continue;
            }
            rows.push((e, prim.path.clone(), pw.is_some()));
        }
    }
    let mut vehicles: Vec<(Entity, String)> = Vec::new();
    {
        // `ActuatorPorts` identifies a VEHICLE ROOT here (only a rover root carries
        // one). Deliberately not `DriveMix`: a root whose mix failed to derive still
        // needs to appear in this list, because the re-derive below is exactly what
        // can give it one.
        let mut q = world.query::<(Entity, &UsdPrimPath, &lunco_core::ActuatorPorts)>();
        for (e, prim, _) in q.iter(world) {
            if prim.stage_handle.id() == id {
                vehicles.push((e, prim.path.clone()));
            }
        }
    }
    if rows.is_empty() && vehicles.is_empty() {
        return;
    }

    // 2. Re-read under one short borrow of the `!Send` stage, then release it —
    //    the appliers below mutate the world (same pattern as
    //    `refresh_domes_live`).
    let mut updates: Vec<WheelUpdate> = Vec::new();
    let mut mixes: Vec<(Entity, Option<lunco_mobility::kernels::DriveMix>)> = Vec::new();
    {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(cs) = stages.get(id) else { return };
        let view = cs.view();
        // This runs only for a stage-change resync, so rebuilding its small
        // stage-local topology snapshot is correct and avoids coupling this
        // exclusive live-edit path to the normal projector cache.
        let mut topology = crate::StageJointTopology::default();
        crate::collect_joint_scan_read(&view, &mut topology);
        for (entity, path, physical) in &rows {
            let Ok(sp) = SdfPath::new(path) else { continue };
            let susp = topology
                .wheel_attachment_targets
                .get(path)
                .and_then(|s| SdfPath::new(s).ok());
            let powertrain = match crate::powertrain::find_for_wheel(&view, &sp) {
                Ok(powertrain) => powertrain,
                Err(missing) => {
                    warn!(
                        "[wheel resync] {} names an invalid or under-authored motor; powertrain attributes to restore {:?} — keeping the spawned values",
                        path,
                        missing
                    );
                    continue;
                }
            };
            match WheelParams::read(&view, &sp, susp.as_ref(), powertrain.as_ref()) {
                Ok(params) => {
                    let (max_steer_angle, ackermann_strength) = match crate::steering_vehicle_of(
                        &view, path,
                    ) {
                        Some(vehicle) => match crate::steering_vehicle_params(&view, &vehicle) {
                            Ok((max, strength)) => (Some(max), strength),
                            Err(reason) => {
                                warn!(
                                    "[wheel resync] {} has invalid Ackermann steering: {} — keeping the spawned values",
                                    path, reason
                                );
                                continue;
                            }
                        },
                        None => (None, 0.0),
                    };
                    updates.push(WheelUpdate {
                        entity: *entity,
                        physical: *physical,
                        params,
                        max_steer_angle,
                        ackermann_strength,
                    });
                }
                Err(missing) => warn!(
                    "[wheel resync] {} now missing required attrs {:?} — keeping \
                     the spawned values (restore the attrs to re-derive)",
                    path, missing
                ),
            }
        }
        for (e, path) in &vehicles {
            let Ok(sp) = SdfPath::new(path) else { continue };
            mixes.push((*e, crate::derive_drive_mix(&view, &sp, path)));
        }
    }

    // 3. Apply in place. NEVER touch entity existence, `JointCollisionDisabled`,
    //    `Position`, or `UsdSimProcessed`.
    let wheel_count = updates.len();
    for u in &updates {
        if !u.physical {
            if let Some(mut wheel) = world.get_mut::<WheelRaycast>(u.entity) {
                u.params.apply_to_raycast(&mut wheel);
            }
            if let Some(mut susp) = world.get_mut::<Suspension>(u.entity) {
                u.params.apply_to_suspension(&mut susp);
            }
            if let (Some(susp), Some(mut ray)) = (
                u.params.suspension,
                world.get_mut::<avian3d::prelude::RayCaster>(u.entity),
            ) {
                ray.origin = DVec3::new(
                    0.0,
                    lunco_mobility::strut_offset(susp.rest_length, u.params.radius),
                    0.0,
                );
                ray.max_distance = susp.rest_length;
            }
            if let (Some(lock), Some(mut steer)) = (
                u.max_steer_angle,
                world.get_mut::<SteeringActuator>(u.entity),
            ) {
                steer.max_steer_angle = lock;
                steer.ackermann_strength = u.ackermann_strength;
            }
            continue;
        }

        // Physical wheel: body-side numbers…
        let (old_radius, old_width, axis_rot) = match world.get::<crate::PhysicalWheel>(u.entity) {
            Some(pw) => (pw.wheel_radius, pw.wheel_width, pw.axis_rot),
            None => continue,
        };
        if let Some(mut pw) = world.get_mut::<crate::PhysicalWheel>(u.entity) {
            pw.wheel_radius = u.params.radius as f32;
            pw.wheel_width = u.params.width as f32;
        }
        if let Some(mut density) = world.get_mut::<ColliderDensity>(u.entity) {
            density.0 = u.params.wheel_density();
        }
        if let Some(mut friction) = world.get_mut::<Friction>(u.entity) {
            friction.dynamic_coefficient = u.params.friction_mu;
            friction.static_coefficient = u.params.friction_mu;
        }
        if let Some(mut tire) = world.get_mut::<JointedWheelTire>(u.entity) {
            tire.radius = u.params.radius;
            tire.axle_inertia = u.params.axle_inertia();
            tire.slip_stiffness = u.params.slip_stiffness;
            tire.cornering_stiffness = u.params.cornering_stiffness;
            tire.min_validated_speed = u.params.min_validated_speed;
            tire.friction_mu = u.params.friction_mu;
            tire.bearing_damping = u.params.bearing_damping;
            tire.axle_axis_local = u.params.axle_axis;
        }
        // Keep the physical wheel's tensor in lock-step with the composed
        // standard MOI and motor reflected inertia.  Updating only density
        // would leave an edited `physxVehicleWheel:moi` inert until a scene
        // reload, while the raycast wheel would apply it immediately.
        world.entity_mut(u.entity).insert((
            crate::physical_wheel_angular_inertia(&u.params, axis_rot),
            avian3d::prelude::NoAutoAngularInertia,
        ));
        // …the collider only when radius or width actually moved (a swap
        // mid-contact can pop the rover; accept as an editing-time artifact,
        // don't pay it for unrelated edits).
        if (old_radius as f64 - u.params.radius).abs() > 1e-6
            || (old_width as f64 - u.params.width).abs() > 1e-6
        {
            let radius = u.params.radius;
            let cyl = Collider::cylinder(radius, u.params.width);
            let collider = if axis_rot.abs_diff_eq(Quat::IDENTITY, 1e-5) {
                cyl
            } else {
                Collider::compound(vec![(
                    Position(DVec3::ZERO),
                    Rotation(axis_rot.as_dquat()),
                    cyl,
                )])
            };
            world.entity_mut(u.entity).insert(collider);
        }
        // …and the joint-side numbers, on the synthesized joint whose `body2`
        // is this wheel. The motor is REBUILT from the one definition
        // (`drive_motor`) with its live command preserved —
        // `motor_actuator_system` rewrites `target_velocity` next tick anyway.
        let mut joint_entity: Option<Entity> = None;
        {
            let mut q = world.query::<(Entity, &RevoluteJoint)>();
            for (je, joint) in q.iter(world) {
                if joint.body2 == u.entity {
                    joint_entity = Some(je);
                    break;
                }
            }
        }
        let Some(je) = joint_entity else { continue };
        if let Some(mut joint) = world.get_mut::<RevoluteJoint>(je) {
            let target_velocity = joint.motor.target_velocity;
            let mut motor = u.params.drive_motor();
            motor.target_velocity = target_velocity;
            joint.motor = motor;
        }
        if let Some(mut motor) = world.get_mut::<MotorActuator>(je) {
            motor.max_omega = u.params.max_rotation_speed;
            motor.peak_torque = u.params.peak_torque;
            motor.brake_torque = u.params.brake_torque_max;
        }
        if let (Some(lock), Some(mut steer)) =
            (u.max_steer_angle, world.get_mut::<SteeringActuator>(je))
        {
            steer.max_steer_angle = lock;
            steer.ackermann_strength = u.ackermann_strength;
        }
    }
    for (e, mix) in mixes {
        if let Some(mix) = mix {
            world.entity_mut(e).insert(mix);
        } else {
            world
                .entity_mut(e)
                .remove::<lunco_mobility::kernels::DriveMix>();
        }
    }
    info!(
        "[wheel resync] stage {:?}: re-derived {} wheel(s), {} vehicle root(s) in place",
        id,
        wheel_count,
        vehicles.len()
    );
}
