use std::path::PathBuf;

fn model(path: &str) -> String {
    std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/models/LunCo")
            .join(path),
    )
    .unwrap()
}

#[test]
fn battery_discharge_current_reduces_soc() {
    let source = model("Electrical/Battery.mo");
    assert!(
        source.contains("der(soc) = p.i / (capacity * 3600.0);"),
        "Battery Pin.i is negative while supplying positive-current loads, so discharge must reduce SoC"
    );
    assert!(
        source.contains("+ p.i * R_internal"),
        "negative discharge current must lower, not raise, terminal voltage"
    );
}

#[test]
fn mass_memory_converts_gigabits_to_gigabytes() {
    let source = model("Storage/MassMemory.mo");
    assert!(
        source.contains("(write_rate_gbps - read_rate_gbps) / 8.0"),
        "MassMemory state is GB while its rates are Gbit/s"
    );
}

#[test]
fn lander_owns_touchdown_continuity_and_controller_inertia_inputs() {
    let source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/models/Lander.mo"),
    )
    .unwrap();
    assert!(source.contains("input Real controller_inertia_xx"));
    assert!(source.contains("input Real controller_inertia_yy"));
    assert!(source.contains("input Real controller_inertia_zz"));
    assert!(source.contains("output Real desired_tilt_x"));
    assert!(source.contains("output Real desired_tilt_z"));
    assert!(source.contains("output Real landing_contact"));
    assert!(source.contains("output Real engine_cutoff_contact"));
    assert!(source.contains("touchdown_ground_speed_mps = 0.05"));
    assert!(source.contains("touchdown_angular_speed_rad_s = 0.005"));
    assert!(source.contains("engine_cutoff_ground_speed_mps = 0.08"));
    assert!(source.contains("touchdown_min_upright_axis_y = 0.9"));
    assert!(source.contains("all_legs_contact = noEvent(if leg_contact_px >= 0.5"));
    assert!(source.contains("minimum_leg_compression = min("));
    assert!(!source.contains("touchdown_min_compression_m"));
    assert!(source.contains("suspension_rate_gate = noEvent(if maximum_leg_speed"));
    assert!(source.contains("engine_cutoff_contact = pad_contact_phase * angular_speed_gate"));
    assert!(source.contains("settled_touchdown_target = landing_contact"));
    assert!(source.contains("navigation_velocity_x"));
    assert!(!source.contains("AboveThreshold"));
}
