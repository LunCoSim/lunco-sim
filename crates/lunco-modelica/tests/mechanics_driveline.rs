//! `LunCo.Mechanics` — the rotational domain, and the connector that makes a
//! driveline composable.
//!
//! `Electrical` has had `Pin` and `Thermal` has had `HeatPort` since the
//! beginning, so a battery joins a motor acausally and the node balances itself.
//! Torque had no such connector, so the one coupling every rover depends on —
//! motor to gearbox to wheel — was spelled as a CAUSAL wire
//! (`Gearbox.inputs:torque.connect = Motor.outputs:torque`). A causal wire
//! carries torque one way and cannot carry speed back, so the motor never sees
//! its own shaft speed, which is why its torque-speed curve had to live outside
//! the model entirely.
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

/// The point of the whole package: rotor — gearbox — wheel, joined at flanges,
/// has to be a solvable system. If `tau` were a plain `Real` instead of a
/// `flow`, the node would impose equality rather than a sum-to-zero and this
/// composition would be structurally singular.
/// Torque enters through a `Torque` source, NOT by writing an equation against
/// `rotor.flange_a.tau`. An unconnected flow variable already carries an implicit
/// `= 0`, so a hand-written equation on one is a SECOND equation for the same
/// unknown — the first version of this test wrote two of them and rumoca
/// correctly reported `19 equations, 17 unknowns (balance = 2)`. `wheel.flange_b`
/// is deliberately left open for the same reason: the free end needs no equation
/// because it already has one.
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
        "rotor-gearbox-wheel should compose through Flange, got: {err:?}"
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

/// Source-level intent, guarded the same way `domain_library_semantics` guards
/// the other domains — cheap, and it catches an edit that silently changes what
/// the connector MEANS.
#[test]
fn flange_shares_angle_and_flows_torque() {
    let flange = package_member("Mechanics/Flange.mo");
    assert!(
        flange.contains("Real phi"),
        "angle is the shared potential — everything on one shaft turns together"
    );
    assert!(
        flange.contains("flow Real tau"),
        "torque MUST be a `flow` or the node imposes equality instead of Newton's third law"
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
        gear.contains("ratio * eta * flange_a.tau")
            && gear.contains("flange_b.tau = -max("),
        "efficiency applies on the torque path and the authored output rating limits it"
    );
}
