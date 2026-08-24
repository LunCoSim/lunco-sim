//! `LunCo.Mechanics` — the rotational domain, and the connector that makes a
//! driveline composable.
//!
//! `Electrical` has had `Pin` and `Thermal` has had `HeatPort` since the
//! beginning, so a battery joins a motor acausally and the node balances itself.
//! Torque is the generic causal-to-acausal mechanical boundary: an electrical
//! motor measures the mechanical speed at the co-simulation boundary, and its
//! solved torque enters the rotational network through `Torque`.
//!
//! These tests pin the two properties that claim is resting on: the members
//! compile as package members, and a driveline built from them with `connect()`
//! actually solves — i.e. rumoca balances `flow Real tau` at the node rather
//! than leaving the composition structurally singular.

use lunco_modelica::ModelicaCompiler;

fn package_member(suffix: &str) -> String {
    lunco_assets::models::package_files("LunCo")
        .into_iter()
        .find(|(path, _)| path.ends_with(suffix))
        .map(|(_, src)| src)
        .unwrap_or_else(|| panic!("{suffix} is part of the shipped LunCo package"))
}

fn compiles(name: &str, suffix: &str) {
    let mut compiler = ModelicaCompiler::new();
    let err = compiler
        .compile_str(
            name,
            &package_member(suffix),
            &format!("lunco://models/LunCo/{suffix}"),
        )
        .err();
    assert!(err.is_none(), "{name} should compile, got: {err:?}");
}

#[test]
fn mechanics_members_compile() {
    compiles("Inertia", "Mechanics/Inertia.mo");
    compiles("GearRatio", "Mechanics/GearRatio.mo");
    compiles("BearingFriction", "Mechanics/BearingFriction.mo");
    compiles("Torque", "Mechanics/Torque.mo");
    compiles("AvianShaft", "Mechanics/AvianShaft.mo");
    compiles("DCMotor", "Electrical/DCMotor.mo");
}

/// The point of the whole package: torque source — gearbox — Avian shaft,
/// joined at flanges, has to be a solvable system. If `tau` were a plain `Real`
/// instead of a `flow`, the node would impose equality rather than a sum-to-zero
/// and this composition would be structurally singular.
const DRIVELINE: &str = r#"
model DrivelineSmoke
  LunCo.Mechanics.Torque src;
  LunCo.Mechanics.Inertia rotor(J = 0.00012);
  LunCo.Mechanics.GearRatio gear(ratio = 200.0, eta = 0.85);
  LunCo.Mechanics.Inertia wheel(J = 8.0);
  input Real tau_in;
  output Real w_wheel;
equation
  src.tau_ref = tau_in;
  connect(src.flange, rotor.flange_a);
  connect(rotor.flange_b, gear.flange_a);
  connect(gear.flange_b, wheel.flange_a);
  w_wheel = wheel.w;
end DrivelineSmoke;
"#;

#[test]
fn a_driveline_composes_through_flanges() {
    let mut compiler = ModelicaCompiler::new();
    let result = compiler.compile_str(
        "DrivelineSmoke",
        DRIVELINE,
        "lunco://models/DrivelineSmoke.mo",
    );
    let err = result.as_ref().err();
    assert!(
        err.is_none(),
        "torque-source-gearbox-wheel should compose through Flange, got: {err:?}"
    );

    let dae = result.unwrap();
    let names: Vec<String> = dae
        .dae
        .variables
        .outputs
        .iter()
        .chain(dae.dae.variables.algebraics.iter())
        .chain(dae.dae.variables.states.iter())
        .map(|(name, _)| name.to_string())
        .collect();
    assert!(
        names
            .iter()
            .any(|n| n == "w_wheel" || n.ends_with("w_wheel")),
        "the wheel speed must be a solved variable; solved = {names:?}"
    );
}

#[test]
fn flange_shares_angle_and_flows_torque() {
    let flange = package_member("Mechanics/Flange.mo");
    assert!(
        flange.contains("Real phi"),
        "angle is the shared potential — everything on one shaft turns together"
    );
    assert!(
        flange.contains("flow Real tau"),
        "torque MUST be a `flow` or the node imposes equality instead of a sum-to-zero"
    );
}

#[test]
fn gearbox_loses_torque_to_friction_but_never_revolutions() {
    let gear = package_member("Mechanics/GearRatio.mo");
    assert!(
        gear.contains("flange_a.phi = ratio * flange_b.phi"),
        "the speed relation is exact — a gearbox does not lose revolutions"
    );
    assert!(
        gear.contains("ratio * eta * flange_a.tau") && gear.contains("flange_b.tau = -max("),
        "efficiency applies on the torque path and the authored output rating limits it"
    );
}

/// Electrical power and mechanical reaction must meet at one authored causal
/// boundary. The motor receives the measured shaft speed as a signal and the
/// generic `Torque` member injects its solved torque into the acausal gearbox;
/// no Modelica state is duplicated across the Modelica/Avian partition.
const BATTERY_DRIVELINE: &str = r#"
model BatteryDrivelineSmoke
  LunCo.Electrical.Battery battery(soc_init = 0.0, voltage_nom = 28.0);
  LunCo.Electrical.DCMotor motor;
  LunCo.Mechanics.Torque source;
  LunCo.Mechanics.GearRatio gear;
  LunCo.Mechanics.AvianShaft shaft;
  input Real demand;
  input Real speed;
  output Real torque;
equation
  motor.demand = demand;
  motor.speed = speed;
  source.tau_ref = motor.shaft_torque;
  shaft.speed = speed;
  connect(battery.p, motor.p);
  connect(source.flange, gear.flange_a);
  connect(gear.flange_b, shaft.flange);
  torque = shaft.torque;
end BatteryDrivelineSmoke;
"#;

#[test]
fn battery_driveline_boundary_is_structurally_solvable() {
    let mut compiler = ModelicaCompiler::new();
    let dae = compiler
        .compile_str(
            "BatteryDrivelineSmoke",
            BATTERY_DRIVELINE,
            "lunco://models/BatteryDrivelineSmoke.mo",
        )
        .expect("battery driveline compiles");
    let stepper = rumoca_sim::SimulationSession::new(&dae.dae, rumoca_sim::SimOptions::default());
    assert!(
        stepper.is_ok(),
        "battery driveline should lower into a solver: {:?}",
        stepper.err()
    );
}

#[test]
fn empty_battery_initialization_preserves_authored_boundary() {
    let mut compiler = ModelicaCompiler::new();
    let dae = compiler
        .compile_str(
            "BatteryEmptyInitialization",
            r#"
model BatteryEmptyInitialization
  LunCo.Electrical.Battery battery(soc_init = 0.0, voltage_nom = 28.0);
  LunCo.Electrical.DCMotor motor;
  input Real demand;
  input Real speed;
equation
  motor.demand = demand;
  motor.speed = speed;
  connect(battery.p, motor.p);
end BatteryEmptyInitialization;
"#,
            "lunco://models/BatteryEmptyInitialization.mo",
        )
        .expect("empty battery composition compiles");
    let session = rumoca_sim::SimulationSession::new(
        &dae.dae,
        rumoca_sim::SimOptions {
            t_start: 0.0,
            t_end: 1.0,
            ..rumoca_sim::SimOptions::default()
        },
    )
    .expect("empty battery composition lowers into a solver");
    let state = session.state().expect("initial state is readable");
    let soc = state
        .values
        .get("battery.soc")
        .copied()
        .expect("battery SOC is visible at the co-simulation boundary");
    assert_eq!(
        soc, 0.0,
        "the solver must preserve an authored zero storage state during initialization"
    );
}
