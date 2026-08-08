//! Sensor boundary contracts.
//!
//! Avian provides primitive state and raw query observations.  The reusable
//! Modelica models are the only place where those facts become IMU, altimeter,
//! or attitude-estimator signals.

use lunco_modelica::ModelicaCompiler;

fn model(path: &str) -> String {
    lunco_assets::models::package_files("LunCo")
        .into_iter()
        .find(|(candidate, _)| candidate.ends_with(path))
        .map(|(_, source)| source)
        .unwrap_or_else(|| panic!("missing shipped Modelica model {path}"))
}

fn compiles(name: &str, source: &str) {
    ModelicaCompiler::new()
        .compile_str(name, source, &format!("lunco://models/LunCo/{name}.mo"))
        .unwrap_or_else(|error| panic!("{name} must compile: {error}"));
}

fn compiles_and_steps(name: &str, source: &str) {
    let mut compiler = ModelicaCompiler::new();
    let dae = compiler
        .compile_str(name, source, &format!("lunco://models/LunCo/{name}.mo"))
        .unwrap_or_else(|error| panic!("{name} must compile: {error}"));
    let mut stepper = rumoca_sim::SimulationSession::new(
        &dae.dae,
        rumoca_sim::SimOptions {
            t_end: 10.0,
            ..Default::default()
        },
    )
    .unwrap_or_else(|error| panic!("{name} must create a live stepper: {error}"));
    stepper
        .step(1.0 / 60.0)
        .unwrap_or_else(|error| panic!("{name} must advance one live step: {error}"));
}

#[test]
fn imu_converts_raw_avian_kinematics_and_environment_gravity() {
    let source = model("Sensors/IMUSensor.mo");
    for input in [
        "raw_acceleration_x",
        "raw_acceleration_y",
        "raw_acceleration_z",
        "raw_angvel_x",
        "raw_angvel_y",
        "raw_angvel_z",
        "raw_quat_w",
        "gravity_world_x",
        "gravity_world_y",
        "gravity_world_z",
    ] {
        assert!(source.contains(input), "IMU must consume {input}");
    }
    for output in [
        "specific_force_x",
        "specific_force_y",
        "specific_force_z",
        "gyro_x",
    ] {
        assert!(source.contains(output), "IMU must expose {output}");
    }
    assert!(!source.contains("accel_x_true"));
    assert!(!source.contains("ground-truth"));
    assert!(
        source.contains("gyro_transform") && source.contains("body_frame_z"),
        "IMU must rotate Avian world angular velocity into body-frame gyro rates"
    );
    assert!(
        source.contains("gravity_transform")
            && source.contains("force_transform.body_frame_y - gravity_transform.body_frame_y"),
        "IMU must transform world acceleration and world gravity before computing body specific force"
    );
    assert!(!source.contains("total_accel_y - gravity_y"));
    assert!(!source.contains("gyro_body_x"));
    compiles_and_steps("IMUSensor", &source);
}

#[test]
fn filtered_signal_is_a_reusable_stateful_measurement_boundary() {
    let source = model("Sensors/FilteredSignal.mo");
    assert!(source.contains("der(state)"));
    assert!(source.contains("sample_valid"));
    assert!(source.contains("acquisition"));
    assert!(source.contains("initial equation") && source.contains("state = u"));
    compiles_and_steps("FilteredSignal", &source);
}

#[test]
fn altimeter_converts_a_raw_ray_observation_without_a_fallback() {
    let source = model("Sensors/Altimeter.mo");
    for input in ["ray_distance_m", "ray_hit_valid"] {
        assert!(source.contains(input), "altimeter must consume {input}");
    }
    for output in [
        "range_m",
        "range_rate_mps",
        "range_valid",
        "range_confidence",
    ] {
        assert!(source.contains(output), "altimeter must expose {output}");
    }
    assert!(!source.contains("alt_true_m"));
    assert!(!source.contains("IdealAltitude"));
    assert!(!source.contains("rangeOutOfRangeMode"));
    compiles_and_steps("Altimeter", &source);
}

#[test]
fn filtered_derivative_is_a_reusable_stateful_sensor_boundary() {
    let source = model("Sensors/FilteredDerivative.mo");
    assert!(source.contains("der(state)"));
    assert!(source.contains("sample_valid"));
    assert!(source.contains("initial equation") && source.contains("state = u"));
    assert!(!source.contains("reinit(state, u)"));
    assert!(!source.contains("initial_value"));
    assert!(!source.contains("der(u)"));
    compiles_and_steps("FilteredDerivative", &source);
}

#[test]
fn attitude_reference_uses_imu_measurements_not_body_truth() {
    let source = model("Sensors/AttitudeReference.mo");
    for input in [
        "attitude_quat_w",
        "attitude_quat_x",
        "attitude_quat_y",
        "attitude_quat_z",
        "desired_tilt_x",
        "desired_tilt_z",
    ] {
        assert!(
            source.contains(input),
            "attitude reference must consume {input}"
        );
    }
    assert!(source.contains("FrameVectorTransform"));
    assert!(source.contains("target_transform"));
    assert!(!source.contains("raw_quat"));
    compiles("AttitudeReference", &source);
}

#[test]
fn all_runtime_frame_conversions_use_the_shared_transform() {
    let shared = model("Sensors/FrameVectorTransform.mo");
    for output in [
        "world_frame_x",
        "world_frame_y",
        "world_frame_z",
        "body_frame_x",
        "body_frame_y",
        "body_frame_z",
    ] {
        assert!(
            shared.contains(output),
            "shared transform must expose {output}"
        );
    }
    compiles_and_steps("FrameVectorTransform", &shared);

    for path in [
        "Sensors/IMUSensor.mo",
        "Sensors/Altimeter.mo",
        "Sensors/AttitudeReference.mo",
        "GNC/LanderNavigation.mo",
        "GNC/PositionPID3D.mo",
    ] {
        let source = model(path);
        assert!(
            source.contains("FrameVectorTransform"),
            "{path} must use the shared frame conversion"
        );
        assert!(
            !source.contains("1.0 - 2.0 * (q_"),
            "{path} must not carry a private quaternion rotation matrix"
        );
    }
}
