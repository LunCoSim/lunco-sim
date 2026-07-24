//! Electrical assembly contract: USD carries component instances and topology;
//! runtime projection turns the enclosing Scope into one Modelica DAE.

use lunco_usd_bevy::UsdRead;
use openusd::sdf::Path as SdfPath;
use openusd::usd::{compute_included_paths, Collection, PrimPredicate};
use std::path::PathBuf;

fn compose_battery_skid() -> lunco_usd_bevy::CanonicalStage {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scenes/sandbox/sandbox_scene.usda");
    let stage =
        lunco_usd_bevy::compose_file_to_stage(&path).expect("compose battery-enabled skid rover");
    lunco_usd_bevy::CanonicalStage::from_stage(stage, path.to_string_lossy().to_string())
}

#[test]
fn electrical_scope_contains_program_api_components() {
    let stage = compose_battery_skid();
    let view = stage.view();
    let rover = "/SandboxScene/Skid_Battery_Thermal_1";
    let root = SdfPath::new(&format!("{rover}/Electrical")).unwrap();
    assert_eq!(view.type_name(&root).as_deref(), Some("Scope"));

    assert!(view.has_api_schema(&root, "CollectionAPI:components"));
    let query = Collection::new(root.clone(), "components")
        .compute_membership_query(view.stage())
        .unwrap();
    let members = compute_included_paths(view.stage(), &query, PrimPredicate::DEFAULT).unwrap();
    assert_eq!(members.len(), 5, "electrical working set: {members:?}");
    for name in ["Battery", "Motor_FL", "Motor_FR", "Motor_RL", "Motor_RR"] {
        let path = SdfPath::new(&format!("{rover}/{name}")).unwrap();
        assert!(
            view.has_api_schema(&path, "LunCoProgramAPI"),
            "{path} must apply the program capability"
        );
        assert!(
            view.asset(&path, "info:sourceAsset").is_some(),
            "{path} must name its Modelica class source"
        );
    }
}

#[test]
fn electrical_topology_uses_modelica_connector_connections() {
    let stage = compose_battery_skid();
    let view = stage.view();
    let rover = "/SandboxScene/Skid_Battery_Thermal_1";
    for name in ["Motor_FL", "Motor_FR", "Motor_RL", "Motor_RR"] {
        let path = SdfPath::new(&format!("{rover}/{name}")).unwrap();
        let targets = view.connections(&path, "connectors:p");
        assert_eq!(
            targets.len(),
            1,
            "{path} must have exactly one bus connection"
        );
        assert!(
            targets[0].to_string().ends_with("/Battery.connectors:p"),
            "unexpected target on {path}: {:?}",
            targets
        );
    }
}

#[test]
fn electrical_scope_exposes_only_causal_cosim_boundary() {
    let stage = compose_battery_skid();
    let view = stage.view();
    let root = SdfPath::new("/SandboxScene/Skid_Battery_Thermal_1/Electrical").unwrap();
    let attrs = view.attr_names(&root);
    for port in ["inputs:drive_left", "inputs:drive_right", "outputs:soc"] {
        assert!(attrs.iter().any(|attr| attr == port), "missing {port}");
    }
    assert!(!attrs.iter().any(|attr| attr.starts_with("connectors:")));
}
