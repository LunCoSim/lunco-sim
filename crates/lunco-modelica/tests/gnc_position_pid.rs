//! Regression coverage for the sensor-driven lander guidance component.
//!
//! The USD component wires these outputs into the airframe. A source edit that
//! parses but is structurally unbalanced must therefore fail this test before it
//! reaches a live scene.

use lunco_modelica::ModelicaCompiler;
use rumoca_sim::{SimOptions, SimulationSession};

fn position_pid_source() -> String {
    lunco_assets::models::package_files("LunCo")
        .into_iter()
        .find(|(path, _)| path.ends_with("GNC/PositionPID3D.mo"))
        .map(|(_, source)| source)
        .expect("LunCo.GNC.PositionPID3D is part of the shipped package")
}

fn pid_axis_source() -> String {
    lunco_assets::models::package_files("LunCo")
        .into_iter()
        .find(|(path, _)| path.ends_with("GNC/PIDAxis.mo"))
        .map(|(_, source)| source)
        .expect("LunCo.GNC.PIDAxis is part of the shipped package")
}

#[test]
fn pid_axis_compiles_and_steps_as_a_reusable_controller() {
    let mut compiler = ModelicaCompiler::new();
    let dae = compiler
        .compile_str(
            "PIDAxis",
            &pid_axis_source(),
            "lunco://models/LunCo/GNC/PIDAxis.mo",
        )
        .expect("PIDAxis must compile");
    let mut stepper = SimulationSession::new(
        &dae.dae,
        SimOptions {
            t_end: 10.0,
            ..Default::default()
        },
    )
    .expect("PIDAxis must create a live stepper");
    stepper
        .step(1.0 / 60.0)
        .expect("PIDAxis must advance one live step");
}

#[test]
fn position_pid3d_compiles_with_live_actuator_outputs() {
    let mut compiler = ModelicaCompiler::new();
    let dae = compiler
        .compile_str(
            "PositionPID3D",
            &position_pid_source(),
            "lunco://models/LunCo/GNC/PositionPID3D.mo",
        )
        .expect("sensor-driven PositionPID3D must be structurally balanced");

    let outputs: Vec<String> = dae
        .dae
        .variables
        .outputs
        .keys()
        .map(ToString::to_string)
        .collect();
    for expected in [
        "throttle_cmd",
        "pitch_cmd",
        "roll_cmd",
        "yaw_cmd",
        "descent_rate_command",
    ] {
        assert!(
            outputs
                .iter()
                .any(|name| name == expected || name.ends_with(expected)),
            "PositionPID3D must expose `{expected}` to USD, got {outputs:?}"
        );
    }

    let mut stepper = SimulationSession::new(
        &dae.dae,
        SimOptions {
            t_end: 10.0,
            ..Default::default()
        },
    )
    .expect("PositionPID3D must create a live stepper");
    stepper
        .step(1.0 / 60.0)
        .expect("PositionPID3D must advance one live step");
}
