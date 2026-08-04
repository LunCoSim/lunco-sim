//! The projection READER, against real composed USD.
//!
//! Every other projection test builds a `DomainNetwork` in Rust, which cannot
//! see the layer this actually fails in: what composition leaves on the stage.
//! The shipped regression — one member dropping out of a composed collection
//! while a boundary output still named it, which rejected the whole electrical
//! domain of a rover that was otherwise fine — lived entirely here.

use lunco_usd_sim::domain_projection::{emit_modelica, read_network, MemberClasses};
use openusd::sdf::Path as SdfPath;
use std::path::PathBuf;

fn stage(fixture: &str) -> lunco_usd_bevy::CanonicalStage {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    let composed = lunco_usd_bevy::compose_file_to_stage(&path).expect("compose fixture");
    lunco_usd_bevy::CanonicalStage::from_stage(composed, path.to_string_lossy().to_string())
}

fn declared_fixture_classes() -> MemberClasses {
    let mut classes = MemberClasses::default();
    classes.declare(
        "lunco://models/LunCo/Electrical/Battery.mo",
        "LunCo.Electrical.Battery",
    );
    classes.declare(
        "lunco://models/LunCo/Electrical/DCMotor.mo",
        "LunCo.Electrical.DCMotor",
    );
    classes.declare(
        "lunco://models/LunCo/Electrical/SolarPanel.mo",
        "LunCo.Electrical.SolarPanel",
    );
    classes
}

#[test]
fn reads_a_composed_collection_into_one_generated_model() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();

    let network = read_network(&view, &root, &declared_fixture_classes())
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

    let network = read_network(&view, &root, &declared_fixture_classes())
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
fn the_class_a_file_declares_beats_the_one_its_path_implies() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();

    // What `resolve_member_classes` reads out of the `.mo` — here the battery's
    // file declares a class its directory layout does NOT imply, which is what a
    // renamed folder or a hand-written `within` looks like.
    let mut classes = declared_fixture_classes();
    classes.declare(
        "lunco://models/LunCo/Electrical/Battery.mo",
        "Vendor.Power.Cell",
    );

    let network = read_network(&view, &root, &classes)
        .expect("declaring a class is not an authoring error")
        .expect("still a network");
    let source = emit_modelica(&network, "Rig_Electrical_System");
    assert!(
        source.contains("Vendor.Power.Cell Rig_x2f_Battery"),
        "the generated model must instantiate what the FILE declares, not what the asset path \
         implies — a guess here surfaces as `class not found` against source nobody can read:\n{source}"
    );
}

#[test]
fn a_member_whose_class_is_unknown_defers_instead_of_guessing() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();

    // The production default: nothing resolved yet, because no `.mo` has loaded.
    let network = read_network(&view, &root, &MemberClasses::default())
        .expect("waiting is not an error")
        .expect("the scope is still a network");
    assert!(
        network.pending_sources,
        "a network whose member classes are unread must report itself pending, so the projector \
         waits rather than compiling a path-derived guess"
    );
    assert!(
        network.components.is_empty(),
        "a pending network states no members: every conclusion drawn from a partial set — which \
         parts are unwired, which boundary outputs have sources — would be a false authoring error"
    );
}

#[test]
fn rejects_members_whose_opinions_cannot_be_generated() {
    let stage = stage("unusable_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();

    let errors = read_network(&view, &root, &declared_fixture_classes())
        .expect_err("unusable authoring is an error");
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
