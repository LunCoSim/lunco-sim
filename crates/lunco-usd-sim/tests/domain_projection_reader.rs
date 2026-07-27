//! The projection READER, against real composed USD.
//!
//! Every other projection test builds a `DomainNetwork` in Rust, which cannot
//! see the layer this actually fails in: what composition leaves on the stage.
//! The shipped regression — one member dropping out of a composed collection
//! while a boundary output still named it, which rejected the whole electrical
//! domain of a rover that was otherwise fine — lived entirely here.

use lunco_usd_sim::domain_projection::{emit_modelica, read_network};
use openusd::sdf::Path as SdfPath;
use std::path::PathBuf;

fn stage(fixture: &str) -> lunco_usd_bevy::CanonicalStage {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    let composed = lunco_usd_bevy::compose_file_to_stage(&path).expect("compose fixture");
    lunco_usd_bevy::CanonicalStage::from_stage(composed, path.to_string_lossy().to_string())
}

#[test]
fn reads_a_composed_collection_into_one_generated_model() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();

    let network = read_network(&view, &root)
        .expect("a well-formed network is not an error")
        .expect("a scope with a component collection is a network");

    let members: Vec<_> = network
        .components
        .iter()
        .map(|component| component.path.as_str())
        .collect();
    assert_eq!(
        members,
        ["/Rig/Battery", "/Rig/Motor"],
        "the installed-but-unconnected panel is not part of the generated island"
    );
    assert_eq!(
        network.components[0].constants.get("voltage_nom"),
        Some(&24.0),
        "an unconnected input is the model's parameter"
    );

    // Instances are named by their composed path relative to the network root;
    // a member that lives OUTSIDE the scope's subtree (the normal shape — parts
    // hang off the vessel, the scope only collects them) keeps its full path,
    // spelled injectively.
    let source = emit_modelica(&network, "Rig_Electrical_System");
    assert!(source.contains("input Real drive_left;"));
    assert!(
        source.contains("connect(Rig_x2f_Battery.p, Rig_x2f_Motor.p);"),
        "acausal edge missing from:\n{source}"
    );
    assert!(
        source.contains("Rig_x2f_Motor.demand = drive_left;"),
        "boundary input equation missing from:\n{source}"
    );
    assert!(
        source.contains("soc = Rig_x2f_Battery.soc_out;"),
        "boundary output equation missing from:\n{source}"
    );
    assert!(
        source.contains("LunCo.Electrical.Battery Rig_x2f_Battery(voltage_nom = 24)"),
        "component instance + its authored parameter missing from:\n{source}"
    );
}

#[test]
fn a_boundary_output_published_through_an_omitted_part_drops_with_it() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();

    let network = read_network(&view, &root)
        .expect(
            "an unwired part must not reject the network — this is the failure that took a \
             rover's whole electrical domain offline",
        )
        .expect("still a network");

    assert!(network.outputs.contains_key("soc"));
    assert!(
        !network.outputs.contains_key("solar_power"),
        "an output published by an omitted component cannot be generated: {:?}",
        network.outputs
    );
}

#[test]
fn rejects_members_whose_opinions_cannot_be_generated() {
    let stage = stage("unusable_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();

    let errors = read_network(&view, &root).expect_err("unusable authoring is an error");
    assert!(
        errors
            .iter()
            .any(|error| error.path == "/Rig/Battery.info:sourceAsset"
                && error.message.contains("subIdentifier")),
        "a source outside a `models/` root has no derivable class, and the message has to say \
         what to author instead: {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.path == "/Rig/Motor.inputs:demand"),
        "a value that cannot be spelled as a Modelica real is reported against the property \
         that carries it, not left to fail in the compiler: {errors:?}"
    );
}
