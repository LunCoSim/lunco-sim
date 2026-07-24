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
fn signal_checks_are_reusable_and_branch_free() {
    let source = model("Logic/AboveThreshold.mo");
    assert!(source.contains("input Real value"));
    assert!(source.contains("parameter Real threshold"));
    assert!(source.contains("max(0.0, min(1.0,"));
    assert!(!source.contains(" if "));
    assert!(!source.contains("when "));
}

#[test]
fn lander_composes_touchdown_from_the_shared_threshold_check() {
    let source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/models/Lander.mo"),
    )
    .unwrap();
    assert!(source.contains("LunCo.Logic.AboveThreshold touchdown_check"));
    assert!(source.contains("touchdown = touchdown_check.active;"));
}
