//! # Powertrain — the motor and gearbox behind a wheel
//!
//! A wheel is geometry, mass, moment of inertia and a tire contact patch. What
//! *turns* it is a motor, through a reduction. Both are real parts
//! (`components/mobility/{motor,gearbox}.usda`) with mass, their own ports and
//! optionally their own Modelica models, because that is what they are on a real
//! machine: you select them, inspect them, swap them, and model their heat.
//!
//! This module reads that chain into one [`PowertrainParams`] and reduces it to the
//! three numbers the wheel dynamics actually need — axle torque, axle no-load speed,
//! and reflected inertia.
//!
//! ## Why this exists at all
//!
//! The former wheel-local speed and torque fields duplicated one physical quantity
//! across the two realizations. One motor/gearbox chain now owns the curve, and one
//! wheel reader reduces it to axle torque, speed, and reflected inertia. Parity is
//! therefore structural rather than two values a human has to keep equal.
//!
//! ## The reduction, and why a linear curve is exact
//!
//! `τ(ω) = stallTorque · (1 − ω/noLoadSpeed)`, clamped at zero. This is not a
//! simplification of a DC motor — it *is* the brushed-DC / FOC-BLDC characteristic
//! that falls out of `τ = k(V − kω)/R`. A sampled torque curve would buy nothing here
//! and cost an interpolation rule, a units convention, and the Inspector sliders that
//! `customData{min,max}` gives scalars for free. A motor that genuinely is not linear
//! (stepper, field-weakening) authors a `LunCoProgramAPI` child that computes torque
//! instead — a program beats scalars for the same reason a wired port beats a constant.
//!
//! ## Optional by construction
//!
//! No gearbox arc means direct drive: ratio 1, efficiency 1. Nothing branches on
//! "has a gearbox" — the identity reduction is the absence of the part.

use lunco_usd_bevy::{SdfPath, UsdRead};

/// A topology error that makes a wheel's drivetrain unsafe to project.
///
/// A wheel may have several motors, but only when they are an explicitly
/// compatible parallel group: every motor must receive the wheel's one drive
/// command, every motor must have at most one reduction stage, and every
/// reduced shaft must have the same no-load speed.  The runtime then sums the
/// independent axle torque and reflected inertia contributions.  Anything else
/// is ambiguous, so the wheel is rejected instead of depending on traversal
/// order.
#[derive(Clone, Debug)]
pub enum PowertrainError {
    InvalidMotor {
        motor: SdfPath,
        attributes: Vec<&'static str>,
    },
    MissingDriveSource {
        wheel: SdfPath,
    },
    MotorDemandMismatch {
        wheel: SdfPath,
        motor: SdfPath,
        source: Option<String>,
        expected: String,
    },
    AmbiguousConnection {
        prim: SdfPath,
        attribute: String,
        sources: Vec<String>,
    },
    MultipleDrivenWheels {
        motor: SdfPath,
        wheels: Vec<SdfPath>,
    },
    MultipleGearboxes {
        motor: SdfPath,
        gearboxes: Vec<SdfPath>,
    },
    IncompatibleNoLoadSpeeds {
        wheel: SdfPath,
        speeds: Vec<(SdfPath, f64)>,
    },
}

/// A wheel's motor + reduction, read from the parts that compose onto the vessel.
#[derive(Clone, Copy, Debug)]
pub struct PowertrainParams {
    /// `lunco:motor:stallTorque` — shaft torque at zero speed, N·m.
    pub stall_torque: f64,
    /// `lunco:motor:noLoadSpeed` — shaft speed at zero torque, rad/s.
    pub no_load_speed: f64,
    /// `lunco:motor:rotorInertia` — kg·m², reflected through the square of the ratio.
    pub rotor_inertia: f64,
    /// `lunco:gearbox:ratio` — reduction, `:1`. 1.0 when there is no gearbox.
    pub ratio: f64,
    /// `lunco:gearbox:efficiency` — 0…1. 1.0 when there is no gearbox.
    pub efficiency: f64,
    /// `lunco:gearbox:maxOutputTorque` — N·m ceiling on the axle. `f64::INFINITY`
    /// when there is no gearbox (a direct-drive motor is limited by its own stall
    /// torque, which is already the cap).
    pub max_output_torque: f64,
}

/// The composed motor part and its immutable drivetrain parameters for one wheel.
/// Keeping the path beside the numbers lets the USD projector bind native runtime
/// readback to the actual motor entity without re-scanning the vessel.
#[derive(Clone, Debug)]
pub struct PowertrainBinding {
    /// Every authored motor in the compatible parallel group.  A wheel with
    /// one motor is simply a one-element group; there is no first-match rule.
    pub motors: Vec<SdfPath>,
    pub params: PowertrainParams,
}

impl PowertrainParams {
    /// Peak torque delivered AT THE AXLE, N·m — what the wheel dynamics see.
    ///
    /// Stall torque geared up, derated by efficiency, then clamped by whatever the
    /// gearbox can actually carry. The clamp is load-bearing: a 1200:1 reduction on a
    /// small motor produces a number that would snap real hardware, and the ceiling is
    /// how an asset says so.
    pub fn axle_peak_torque(&self) -> f64 {
        (self.stall_torque * self.ratio * self.efficiency).min(self.max_output_torque)
    }

    /// No-load speed AT THE AXLE, rad/s — THE top-speed number both wheel
    /// realizations obey (the joint motor targets it; the raycast force rolls off
    /// toward it). One source, so they cannot disagree.
    pub fn axle_no_load_speed(&self) -> f64 {
        if self.ratio > 0.0 {
            self.no_load_speed / self.ratio
        } else {
            0.0
        }
    }

    /// Rotor inertia reflected to the axle, kg·m² — `J · ratio²`.
    ///
    /// Squared, not linear: the rotor spins `ratio` times faster than the axle, and
    /// kinetic energy goes as ω². At the shipped 1200:1 this dominates the wheel's own
    /// ½mr² by orders of magnitude, which is physically right and is why a geared rover
    /// feels heavy to spin up rather than snapping to speed.
    pub fn reflected_inertia(&self) -> f64 {
        self.rotor_inertia * self.ratio * self.ratio
    }
}

/// Read one motor (and its optional gearbox) into a [`PowertrainParams`].
///
/// Returns `Err` naming the missing attributes, collected, so one under-authored motor
/// reports everything wrong with it rather than the first thing. There are NO Rust
/// fallbacks: a motor that does not declare its torque is an asset error, not a motor
/// with a default torque.
pub fn read_powertrain(
    reader: &lunco_usd_bevy::StageView<'_>,
    motor: &SdfPath,
    gearbox: Option<&SdfPath>,
) -> Result<PowertrainParams, Vec<&'static str>> {
    let mut missing: Vec<&'static str> = Vec::new();
    let mut req = |path: &SdfPath, name: &'static str| -> f64 {
        match reader.real(path, name) {
            Some(v) => v,
            None => {
                missing.push(name);
                0.0
            }
        }
    };

    let stall_torque = req(motor, "lunco:motor:stallTorque");
    let no_load_speed = req(motor, "lunco:motor:noLoadSpeed");
    let rotor_inertia = req(motor, "lunco:motor:rotorInertia");

    // Absence of the part IS the identity reduction — no branch, no default value
    // standing in for a missing gearbox.
    let (ratio, efficiency, max_output_torque) = match gearbox {
        Some(g) => (
            req(g, "lunco:gearbox:ratio"),
            req(g, "lunco:gearbox:efficiency"),
            req(g, "lunco:gearbox:maxOutputTorque"),
        ),
        None => (1.0, 1.0, f64::INFINITY),
    };

    // Schema slider hints are authoring guidance, not runtime validation. Reject
    // non-finite or physically meaningless authored values here; otherwise a
    // malformed driven wheel could become a zero-torque wheel or inject NaNs into
    // the fixed-step actuator path. An absent field is already in `missing`, so
    // avoid reporting the same name twice.
    let invalid = |missing: &mut Vec<&'static str>, name: &'static str, ok: bool| {
        if !ok && !missing.contains(&name) {
            missing.push(name);
        }
    };
    invalid(
        &mut missing,
        "lunco:motor:stallTorque",
        stall_torque.is_finite() && stall_torque >= 0.0,
    );
    invalid(
        &mut missing,
        "lunco:motor:noLoadSpeed",
        no_load_speed.is_finite() && no_load_speed > 0.0,
    );
    invalid(
        &mut missing,
        "lunco:motor:rotorInertia",
        rotor_inertia.is_finite() && rotor_inertia >= 0.0,
    );
    if gearbox.is_some() {
        invalid(
            &mut missing,
            "lunco:gearbox:ratio",
            ratio.is_finite() && ratio > 0.0,
        );
        invalid(
            &mut missing,
            "lunco:gearbox:efficiency",
            efficiency.is_finite() && (0.0..=1.0).contains(&efficiency),
        );
        invalid(
            &mut missing,
            "lunco:gearbox:maxOutputTorque",
            max_output_torque.is_finite() && max_output_torque >= 0.0,
        );
    }

    if !missing.is_empty() {
        return Err(missing);
    }
    Ok(PowertrainParams {
        stall_torque,
        no_load_speed,
        rotor_inertia,
        ratio,
        efficiency,
        max_output_torque,
    })
}

/// The powertrain driving `wheel`, discovered by searching the wheel's vessel for a
/// motor that names it.
///
/// Returns `Ok(None)` for an undriven wheel — a castor or a trailer wheel is a
/// legitimate thing to author, and it is not an error. A motor that names the
/// wheel but is under-authored returns `Err`, so it cannot silently become an
/// undriven wheel.
///
/// The search ascends from the wheel to its vessel root and scans that subtree, rather
/// than looking at siblings: on a rocker-bogie the motors are children of the ARM
/// bodies so they swing with the suspension, so a motor and the wheel it turns are not
/// siblings and need not share a parent.
pub fn find_for_wheel(
    reader: &lunco_usd_bevy::StageView<'_>,
    wheel: &SdfPath,
) -> Result<Option<PowertrainParams>, PowertrainError> {
    find_binding_for_wheel(reader, wheel).map(|binding| binding.map(|binding| binding.params))
}

/// Resolve every composed motor that drives `wheel` and its immutable parameters.
/// The relationship is the sole identity source; the motors and wheel need not be
/// siblings, and their display names play no part in the resolution. A compatible
/// group is reduced to one equivalent axle model; an incompatible group is an
/// authoring error rather than a first-match choice.
pub fn find_binding_for_wheel(
    reader: &lunco_usd_bevy::StageView<'_>,
    wheel: &SdfPath,
) -> Result<Option<PowertrainBinding>, PowertrainError> {
    let Some(root) = vessel_root(wheel) else {
        return Ok(None);
    };
    let mut motors = Vec::new();
    collect_by_api(reader, &root, "LunCoMotorAPI", &mut motors);

    let want = wheel.as_str();
    let mut matched = Vec::new();
    for motor in motors {
        let wheels = reader.rel_targets(&motor, "lunco:motor:drivenWheel");
        if wheels.len() > 1 {
            return Err(PowertrainError::MultipleDrivenWheels { motor, wheels });
        }
        if wheels.first().map(SdfPath::as_str) == Some(want) {
            matched.push(motor);
        }
    }
    let motors = matched;
    if motors.is_empty() {
        return Ok(None);
    }

    // Parallel motors are a real drivetrain arrangement, but the current wheel
    // projection has one command surface.  Require every source to be exactly
    // the wheel's authored drive connection, so two independent commands can
    // never be silently merged into one scalar actuator.
    let expected = one_connection(reader, wheel, "inputs:drive")?.ok_or_else(|| {
        PowertrainError::MissingDriveSource {
            wheel: wheel.clone(),
        }
    })?;

    // The gearbox is whichever one takes its torque FROM this motor. Derived from the
    // connection, not from a naming convention — `Gearbox_FL` next to `Motor_FL` is a
    // readability nicety, never the binding.
    let mut boxes = Vec::new();
    collect_by_api(reader, &root, "LunCoGearboxAPI", &mut boxes);
    let mut sources = Vec::with_capacity(motors.len());
    for motor in &motors {
        let source = one_connection(reader, motor, "inputs:demand")?;
        // The wheel may consume the motor's authored mechanical command output
        // instead of the raw vehicle command. The connection itself is the
        // contract: accept any output authored on this motor prim, without
        // teaching Rust a model-specific output name. The Modelica projection
        // publishes the declared output through the normal generated-domain
        // boundary, and USD selects which output feeds the wheel.
        let wheel_uses_motor_output =
            expected
                .rsplit_once('.')
                .is_some_and(|(source_prim, source_property)| {
                    source_prim == motor.as_str() && source_property.starts_with("outputs:")
                });
        if source.as_deref() != Some(expected.as_str()) && !wheel_uses_motor_output {
            return Err(PowertrainError::MotorDemandMismatch {
                wheel: wheel.clone(),
                motor: motor.clone(),
                source,
                expected,
            });
        }

        let motor_out = format!("{}.outputs:torque", motor.as_str());
        let mut attached = Vec::new();
        for gearbox in &boxes {
            let Some(source) = one_connection(reader, gearbox, "inputs:torque")? else {
                continue;
            };
            if source == motor_out {
                attached.push(gearbox.clone());
            }
        }
        if attached.len() > 1 {
            return Err(PowertrainError::MultipleGearboxes {
                motor: motor.clone(),
                gearboxes: attached,
            });
        }
        let params = read_powertrain(reader, motor, attached.first()).map_err(|attributes| {
            PowertrainError::InvalidMotor {
                motor: motor.clone(),
                attributes,
            }
        })?;
        sources.push((motor.clone(), params));
    }

    let speeds: Vec<(SdfPath, f64)> = sources
        .iter()
        .map(|(motor, params)| (motor.clone(), params.axle_no_load_speed()))
        .collect();
    let reference = speeds[0].1;
    // This is a dimensional/numerical comparison tolerance, not a dynamics
    // tuning knob: values that differ beyond floating-point authoring noise
    // describe different matched-load operating points and cannot be summed
    // into one linear torque-speed curve.
    let compatible = |speed: f64| (speed - reference).abs() <= 1.0e-9 * reference.abs().max(1.0);
    if speeds.iter().any(|(_, speed)| !compatible(*speed)) {
        return Err(PowertrainError::IncompatibleNoLoadSpeeds {
            wheel: wheel.clone(),
            speeds,
        });
    }

    let params = PowertrainParams::parallel(
        &sources
            .iter()
            .map(|(_, params)| *params)
            .collect::<Vec<_>>(),
    );
    Ok(Some(PowertrainBinding {
        motors: sources.into_iter().map(|(motor, _)| motor).collect(),
        params,
    }))
}

impl PowertrainParams {
    /// Reduce matched motors in parallel to the equivalent axle model consumed
    /// by the two wheel realizations.  Torque and rotor inertia add; the common
    /// reduced no-load speed remains the speed of each source.  This is the
    /// exact sum of independent linear DC motor curves under one shared demand,
    /// not a fitted multiplier.
    fn parallel(sources: &[Self]) -> Self {
        debug_assert!(!sources.is_empty());
        let speed = sources.first().map(Self::axle_no_load_speed).unwrap_or(0.0);
        Self {
            stall_torque: sources.iter().map(Self::axle_peak_torque).sum(),
            no_load_speed: speed,
            rotor_inertia: sources.iter().map(Self::reflected_inertia).sum(),
            ratio: 1.0,
            efficiency: 1.0,
            max_output_torque: f64::INFINITY,
        }
    }
}

/// Ascend to the vessel root — the highest ancestor below the stage root.
fn vessel_root(prim: &SdfPath) -> Option<SdfPath> {
    let s = prim.as_str();
    let mut parts = s.trim_start_matches('/').split('/');
    let first = parts.next()?;
    SdfPath::new(&format!("/{}", first)).ok()
}

/// Recursively gather prims under `root` applying `api`.
fn collect_by_api(
    reader: &lunco_usd_bevy::StageView<'_>,
    root: &SdfPath,
    api: &str,
    out: &mut Vec<SdfPath>,
) {
    for child in reader.children(root) {
        if reader.has_api_schema(&child, api) {
            out.push(child.clone());
        }
        collect_by_api(reader, &child, api, out);
    }
}

/// Read a single-producer connection without silently selecting the first item
/// from a USD fan-in list. Motors, gearboxes, and a wheel's command endpoint are
/// one-to-one topology edges; a multi-source list is an authoring error.
fn one_connection(
    reader: &lunco_usd_bevy::StageView<'_>,
    prim: &SdfPath,
    attribute: &str,
) -> Result<Option<String>, PowertrainError> {
    let sources = reader.connections(prim, attribute);
    match sources.len() {
        0 => Ok(None),
        1 => Ok(sources.into_iter().next()),
        _ => Err(PowertrainError::AmbiguousConnection {
            prim: prim.clone(),
            attribute: attribute.to_string(),
            sources,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{find_binding_for_wheel, PowertrainError, PowertrainParams};
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};
    use openusd::sdf::Path as SdfPath;

    fn source(stall: f64, speed: f64, rotor: f64, ratio: f64, efficiency: f64) -> PowertrainParams {
        PowertrainParams {
            stall_torque: stall,
            no_load_speed: speed,
            rotor_inertia: rotor,
            ratio,
            efficiency,
            max_output_torque: f64::INFINITY,
        }
    }

    #[test]
    fn matched_parallel_motors_sum_axle_torque_and_reflected_inertia() {
        let a = source(1.0, 2400.0, 0.001, 200.0, 0.85);
        let b = source(0.5, 2400.0, 0.002, 200.0, 0.85);
        let group = PowertrainParams::parallel(&[a, b]);

        assert!((group.axle_peak_torque() - 255.0).abs() < 1.0e-9);
        assert!((group.axle_no_load_speed() - 12.0).abs() < 1.0e-9);
        assert!((group.reflected_inertia() - 120.0).abs() < 1.0e-9);
    }

    #[test]
    fn parallel_reduction_preserves_the_linear_torque_speed_curve() {
        let a = source(1.0, 2400.0, 0.001, 200.0, 0.85);
        let b = source(0.5, 2400.0, 0.002, 200.0, 0.85);
        let group = PowertrainParams::parallel(&[a, b]);
        let expected = a.axle_peak_torque() + b.axle_peak_torque();

        assert!((group.axle_peak_torque() * 0.5 - expected * 0.5).abs() < 1.0e-9);
        assert_eq!(group.ratio, 1.0);
        assert_eq!(group.efficiency, 1.0);
    }

    #[test]
    fn rejects_a_second_motor_with_a_different_command_source() {
        // Two motors on one wheel are valid only as a parallel group sharing
        // one authored drive demand. The resolver must reject the ambiguous
        // topology instead of depending on traversal order.
        const SCENE: &str = r#"#usda 1.0
def Xform "Rover" {
    def Xform "DriveA" { float outputs:throttle = 0.0 }
    def Xform "DriveB" { float outputs:throttle = 0.0 }
    def Xform "Wheel" (prepend apiSchemas = ["PhysxVehicleWheelAPI"]) {
        float inputs:drive.connect = </Rover/DriveA.outputs:throttle>
    }
    def Xform "MotorA" (prepend apiSchemas = ["LunCoMotorAPI"]) {
        rel lunco:motor:drivenWheel = </Rover/Wheel>
        float inputs:demand.connect = </Rover/DriveA.outputs:throttle>
        float lunco:motor:stallTorque = 1.0
        float lunco:motor:noLoadSpeed = 2400.0
        float lunco:motor:rotorInertia = 0.001
    }
    def Xform "MotorB" (prepend apiSchemas = ["LunCoMotorAPI"]) {
        rel lunco:motor:drivenWheel = </Rover/Wheel>
        float inputs:demand.connect = </Rover/DriveB.outputs:throttle>
        float lunco:motor:stallTorque = 1.0
        float lunco:motor:noLoadSpeed = 2400.0
        float lunco:motor:rotorInertia = 0.001
    }
}
"#;
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("parallel.usda", SCENE))
            .expect("parallel-motor fixture composes");
        let wheel = SdfPath::new("/Rover/Wheel").unwrap();
        let result = find_binding_for_wheel(&stage.view(), &wheel);
        match result {
            Err(PowertrainError::MotorDemandMismatch { motor, .. }) => {
                assert_eq!(motor.as_str(), "/Rover/MotorB");
            }
            other => panic!("expected demand-source rejection, got {other:?}"),
        }
    }
}
