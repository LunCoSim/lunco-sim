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
    assert!(source.contains("touchdown = 0.5 + 0.5 * touchdown_error"));
    assert!(!source.contains("AboveThreshold"));
}
