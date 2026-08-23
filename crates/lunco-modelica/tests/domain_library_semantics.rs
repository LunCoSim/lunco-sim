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
        source.contains("soc_rate = p.i / (capacity * 3600.0);"),
        "Battery Pin.i is negative while supplying positive-current loads, so discharge must reduce SoC"
    );
    assert!(
        source.contains("der(soc) = max(-soc, min(1.0 - soc, soc_rate));"),
        "finite storage must keep the battery state inside the physical [0, 1] interval"
    );
    assert!(
        source.contains("+ p.i * R_internal"),
        "negative discharge current must lower, not raise, terminal voltage"
    );
}

#[test]
fn electrical_observables_follow_the_physical_power_chain() {
    let solar = model("Electrical/SolarPanel.mo");
    assert!(
        solar.contains("available_power_w = area * efficiency * irradiance * cos_incidence;"),
        "available PV power must come from sunlight and incidence, before bus loading"
    );
    assert!(
        solar.contains("power_out = -p.i * p.v;"),
        "delivered PV power must be the solved terminal current times bus voltage"
    );

    let battery = model("Electrical/Battery.mo");
    assert!(
        battery.contains("net_power_w = p.v * p.i;"),
        "battery net power must use the signed terminal convention"
    );
    assert!(
        battery.contains("charge_power_w = max(0.0, net_power_w);"),
        "positive battery power must be exposed as charging"
    );
    assert!(
        battery.contains("discharge_power_w = max(0.0, -net_power_w);"),
        "negative battery power must be exposed as positive discharge"
    );

    let motor = model("Electrical/DCMotor.mo");
    assert!(
        motor.contains("electrical_power = p.i * p.v;"),
        "motor electrical draw must be solved from its terminal current and voltage"
    );
    assert!(
        motor.contains("heat = max(0.0, electrical_power) * (1.0 - efficiency);"),
        "motor heat must be the electrical loss, not a second synthetic load"
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
    assert!(source.contains("touchdown_ground_speed_mps = 0.15"));
    assert!(source.contains("touchdown_angular_speed_rad_s = 0.005"));
    assert!(source.contains("engine_cutoff_ground_speed_mps = 0.08"));
    assert!(source.contains("touchdown_min_upright_axis_y = 0.9"));
    assert!(source.contains("input Real predicate_transition_band = 1.0e-3"));
    assert!(source.contains("all_legs_contact = min("));
    assert!(source.contains("minimum_leg_compression = min("));
    assert!(!source.contains("touchdown_min_compression_m"));
    assert!(source.contains("suspension_rate_gate = max(0.0, min(1.0,"));
    assert!(!source.contains("noEvent(if"));
    assert!(source.contains("engine_cutoff_contact = pad_contact_phase * angular_speed_gate"));
    assert!(source.contains("settled_touchdown_target = landing_contact"));
    assert!(source.contains("navigation_velocity_x"));
    assert!(!source.contains("AboveThreshold"));
}
