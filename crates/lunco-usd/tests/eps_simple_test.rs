//! The rover's electrical system, as ASSEMBLY.
//!
//! The electrical domain is one acausal circuit solved as one DAE
//! (`rucheyok_electrical.mo`, importing `LunCo.Electrical`), so in USD it is one
//! `LunCoProgram` under a `def Scope "Electrical"` — not a program per component. The
//! panel, pack and motors are geometry; their maths is the circuit's. This checks the
//! assembly USD owns: the domain program is there, bound to its circuit, its parameters
//! are authored, and its boundary ports exist for cosim to wire.
//!
//! What comes out — the power balance, the SoC curve — is the model's, checked by running
//! it (a cosim test), never re-derived here.

use lunco_usd_bevy::{StageView, UsdRead};
use openusd::sdf::Path as SdfPath;
use std::path::PathBuf;

fn assets_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("assets")
}

fn compose_rucheyok() -> lunco_usd_bevy::CanonicalStage {
    let path = assets_root().join("vessels/rovers/rucheyok/rucheyok.usda");
    let stage = lunco_usd_bevy::compose_file_to_stage(&path).expect("compose rucheyok");
    lunco_usd_bevy::CanonicalStage::from_stage(stage, path.to_string_lossy().to_string())
}

const ELECTRICAL: &str = "/Rucheyok/Electrical/System";

/// The electrical domain is one program, bound to the rover's circuit.
#[test]
fn rover_has_one_electrical_program_bound_to_its_circuit() {
    let cs = compose_rucheyok();
    let view = cs.view();
    let prim = SdfPath::new(ELECTRICAL).unwrap();

    let model = view
        .asset(&prim, "info:sourceAsset")
        .expect("the Electrical program must name a Modelica circuit");
    assert!(
        model.ends_with("rucheyok_electrical.mo"),
        "expected the rover's electrical circuit, got {model}"
    );

    // Exactly one — the domain is one DAE, not a program per part.
    let programs: Vec<_> = view
        .prim_paths()
        .into_iter()
        .filter(|p| view.asset(p, "info:sourceAsset").is_some())
        .filter(|p| {
            view.asset(p, "info:sourceAsset")
                .map(|m| m.ends_with("rucheyok_electrical.mo"))
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(programs.len(), 1, "expected one electrical program, got {programs:#?}");
}

/// The circuit's parameters are valued in USD; its boundary ports exist for cosim.
#[test]
fn the_electrical_program_authors_its_parameters_and_boundary() {
    let cs = compose_rucheyok();
    let view = cs.view();
    let prim = SdfPath::new(ELECTRICAL).unwrap();
    let attrs = view.attr_names(&prim);

    // Parameters — the circuit's top-level `parameter Real`s, valued here.
    for p in [
        "inputs:panel_area",
        "inputs:battery_capacity",
        "inputs:motor_rated_power",
    ] {
        assert!(
            view.real(&prim, p).is_some(),
            "the Electrical program must author {p}"
        );
    }

    // Boundary — one shaft-speed port per wheel and the environment feed, wired by cosim.
    for port in [
        "inputs:irradiance",
        "inputs:omega_fl",
        "inputs:omega_fr",
        "inputs:omega_rl",
        "inputs:omega_rr",
        "outputs:soc",
    ] {
        assert!(
            attrs.iter().any(|a| a == port),
            "the Electrical program must expose {port}; has {attrs:?}"
        );
    }
}

/// The power parts are physical components, not carriers of electrical behaviour: they
/// bind NO program. Their maths is the circuit's.
#[test]
fn power_parts_are_geometry_not_programs() {
    let cs = compose_rucheyok();
    let view = cs.view();
    for prim in [
        "/Rucheyok/SolarPanel",
        "/Rucheyok/Battery",
        "/Rucheyok/Wheel_FL",
    ] {
        let p = SdfPath::new(prim).unwrap();
        assert!(
            view.asset(&p, "info:sourceAsset").is_none(),
            "{prim} should be a physical part with no program of its own"
        );
    }
}
