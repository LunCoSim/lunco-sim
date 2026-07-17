//! Simple EPS power flow test without physics mechanics.
//!
//! This test verifies the electrical power system structure
//! and simulates basic power flow using only USD data - no composition needed.

use lunco_usd_bevy::StageView;
use openusd::sdf::Path as SdfPath;
use std::path::PathBuf;

/// Represents a component on the EPS bus
#[derive(Debug)]
struct EPSComponent {
    path: String,
    /// Power generation (W) - positive for generators, negative for loads
    power_watts: f64,
}

/// Simulates EPS power flow
fn simulate_eps_flow(components: &[EPSComponent]) -> PowerBalance {
    let total_generation: f64 = components
        .iter()
        .filter(|c| c.power_watts > 0.0)
        .map(|c| c.power_watts)
        .sum();

    let total_consumption: f64 = components
        .iter()
        .filter(|c| c.power_watts < 0.0)
        .map(|c| c.power_watts.abs())
        .sum();

    PowerBalance {
        generation: total_generation,
        consumption: total_consumption,
        surplus: total_generation - total_consumption,
    }
}

#[derive(Debug)]
struct PowerBalance {
    generation: f64,
    consumption: f64,
    surplus: f64,
}

/// Load the rover, composed so referenced component parameters (e.g. the
/// wheel's `lunco:motorPower` from wheel.usda) surface on the instance prims,
/// and extract its EPS components.
fn load_rover_eps() -> Vec<EPSComponent> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let asset_root = manifest_dir.parent().unwrap().parent().unwrap();
    let usd_path = asset_root.join("assets/vessels/rovers/rucheyok/rucheyok.usda");

    let stage = lunco_usd_bevy::compose_file_to_stage(&usd_path).expect("Failed to compose rover USD");
    let view = StageView::new(&stage);

    let mut components = Vec::new();
    for path in view.prim_paths() {
        // Only components wired onto the EPS bus.
        if !view.rel_targets(&path, "lunco:epsBus").is_empty() {
            components.push(EPSComponent {
                path: path.as_str().to_string(),
                power_watts: get_component_power(&view, &path),
            });
        }
    }

    components
}

/// Power generation (+) / consumption (-) for a composed component prim.
/// Generators author `lunco:nominalPower`; motorized loads author
/// `lunco:motorPower` (composed in from their referenced component).
fn get_component_power(view: &StageView<'_>, path: &SdfPath) -> f64 {
    if let Some(power) = view.value::<f64>(path, "lunco:nominalPower") {
        return power; // Positive = generation
    }
    if let Some(power) = view.value::<f64>(path, "lunco:motorPower") {
        return -power; // Negative = consumption
    }
    0.0
}

#[test]
fn test_eps_components_exist() {
    let components = load_rover_eps();

    // Should have at least: SolarPanel, Battery, 4 Wheels
    assert!(
        components.len() >= 6,
        "Should have at least 6 EPS components, got {}",
        components.len()
    );

    // Check expected components exist
    let paths: Vec<&str> = components.iter().map(|c| c.path.as_str()).collect();
    assert!(
        paths.iter().any(|p| p.contains("SolarPanel")),
        "Should have SolarPanel"
    );
    assert!(
        paths.iter().any(|p| p.contains("Battery")),
        "Should have Battery"
    );
}

#[test]
fn test_eps_power_generation() {
    let components = load_rover_eps();

    // Solar panel should generate power
    let solar = components
        .iter()
        .find(|c| c.path.contains("SolarPanel"))
        .expect("SolarPanel should exist");

    assert!(
        solar.power_watts > 0.0,
        "Solar panel should generate power (positive), got {}",
        solar.power_watts
    );
}

#[test]
fn test_eps_power_consumption() {
    let components = load_rover_eps();

    // Wheels should consume power
    let wheels: Vec<&EPSComponent> = components
        .iter()
        .filter(|c| c.path.contains("Wheel"))
        .collect();

    assert!(
        wheels.len() == 4,
        "Should have 4 wheels, got {}",
        wheels.len()
    );

    for wheel in wheels {
        assert!(
            wheel.power_watts < 0.0,
            "Wheel should consume power (negative), got {}",
            wheel.power_watts
        );
    }
}

#[test]
fn test_eps_power_balance() {
    let components = load_rover_eps();
    let balance = simulate_eps_flow(&components);

    println!("\n=== EPS Power Balance ===");
    println!("  Generation:  {:.1} W", balance.generation);
    println!("  Consumption: {:.1} W", balance.consumption);
    println!("  Surplus:     {:.1} W", balance.surplus);
    println!("========================\n");

    // Solar panel should generate 800W (rover override)
    assert!(
        (balance.generation - 800.0).abs() < 10.0,
        "Solar panel should generate ~800W, got {:.1}",
        balance.generation
    );

    // 4 wheels × 2000W = 8000W consumption
    assert!(
        (balance.consumption - 8000.0).abs() < 100.0,
        "Wheels should consume ~8000W, got {:.1}",
        balance.consumption
    );
}
