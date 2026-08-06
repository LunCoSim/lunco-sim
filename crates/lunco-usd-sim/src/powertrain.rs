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
    pub motor: SdfPath,
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
) -> Result<Option<PowertrainParams>, Vec<&'static str>> {
    find_binding_for_wheel(reader, wheel).map(|binding| binding.map(|binding| binding.params))
}

/// Resolve the composed motor that drives `wheel` and its immutable parameters.
/// The relationship is the sole identity source; the motor and wheel need not be
/// siblings, and their display names play no part in the resolution.
pub fn find_binding_for_wheel(
    reader: &lunco_usd_bevy::StageView<'_>,
    wheel: &SdfPath,
) -> Result<Option<PowertrainBinding>, Vec<&'static str>> {
    let Some(root) = vessel_root(wheel) else {
        return Ok(None);
    };
    let mut motors = Vec::new();
    collect_by_api(reader, &root, "LunCoMotorAPI", &mut motors);

    let want = wheel.as_str();
    let Some(motor) = motors
        .iter()
        .find(|m| reader.rel_target(m, "lunco:motor:drivenWheel").as_deref() == Some(want))
    else {
        return Ok(None);
    };

    // The gearbox is whichever one takes its torque FROM this motor. Derived from the
    // connection, not from a naming convention — `Gearbox_FL` next to `Motor_FL` is a
    // readability nicety, never the binding.
    let mut boxes = Vec::new();
    collect_by_api(reader, &root, "LunCoGearboxAPI", &mut boxes);
    let motor_out = format!("{}.outputs:torque", motor.as_str());
    let gearbox = boxes.iter().find(|g| {
        reader
            .connection_source(g, "inputs:torque")
            .as_deref()
            .map(|t| t == motor_out)
            .unwrap_or(false)
    });

    match read_powertrain(reader, motor, gearbox) {
        Ok(params) => Ok(Some(PowertrainBinding {
            motor: motor.clone(),
            params,
        })),
        Err(missing) => Err(missing),
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
