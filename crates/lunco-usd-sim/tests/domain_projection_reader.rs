//! The projection READER, against real composed USD.
//!
//! Every other projection test builds a `DomainNetwork` in Rust, which cannot
//! see the layer this actually fails in: what composition leaves on the stage.
//! The shipped regression — one member dropping out of a composed collection
//! while a boundary output still named it, which rejected the whole electrical
//! domain of a rover that was otherwise fine — lived entirely here.

use lunco_usd_sim::domain_projection::{network_facts, read_network, MemberClasses};
use openusd::sdf::Path as SdfPath;
use std::path::PathBuf;

fn stage(fixture: &str) -> lunco_usd_bevy::CanonicalStage {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    let composed = lunco_usd_bevy::compose_file_to_stage(&path).expect("compose fixture");
    lunco_usd_bevy::CanonicalStage::from_stage(composed, path.to_string_lossy().to_string())
}

fn fixture_classes() -> MemberClasses {
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
fn reads_a_composed_collection_into_network_facts() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig").unwrap();

    let network = read_network(&view, &root, &fixture_classes())
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

    let facts = network_facts(&network, "Rig_System", Some(&fixture_classes()))
        .expect("network facts are valid");
    assert_eq!(
        facts.get("model_name").and_then(|value| value.as_str()),
        Some("Rig_System")
    );
    assert!(matches!(
        facts.get("connections"),
        Some(lunco_hooks::HookValue::Array(edges)) if !edges.is_empty()
    ));
    assert!(matches!(
        facts.get("boundary_links"),
        Some(lunco_hooks::HookValue::Array(links)) if !links.is_empty()
    ));
    let lunco_hooks::HookValue::Array(boundary_links) = facts.get("boundary_links").unwrap() else {
        panic!("boundary_links is an array");
    };
    assert!(boundary_links.iter().any(|link| {
        link.get("input").and_then(|value| value.as_str()) == Some("drive_left")
            && link.get("target_path").and_then(|value| value.as_str()) == Some("/Rig/Motor")
            && link.get("target_input").and_then(|value| value.as_str()) == Some("demand")
    }));
}

#[test]
fn a_boundary_output_published_through_an_omitted_part_drops_with_it() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig").unwrap();

    let network = read_network(&view, &root, &fixture_classes())
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
fn an_explicitly_resolved_source_class_is_exposed_to_policy_facts() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig").unwrap();

    // What `resolve_member_classes` reads out of the `.mo` — here the battery's
    // file declares a class its directory layout does NOT imply, which is what a
    // renamed folder or a hand-written `within` looks like.
    let mut classes = fixture_classes();
    classes.declare(
        "lunco://models/LunCo/Electrical/Battery.mo",
        "Vendor.Power.Cell",
    );

    let network = read_network(&view, &root, &classes)
        .expect("declaring a class is not an authoring error")
        .expect("still a network");
    let facts = network_facts(&network, "Rig_System", Some(&classes))
        .expect("network facts are valid");
    let Some(lunco_hooks::HookValue::Array(components)) = facts.get("components") else {
        panic!("component facts");
    };
    let battery = components
        .iter()
        .find(|component| {
            component.get("path").and_then(|value| value.as_str()) == Some("/Rig/Battery")
        })
        .expect("battery facts");
    assert!(
        battery.get("class").and_then(|value| value.as_str()) == Some("Vendor.Power.Cell"),
        "the resolved source class must reach the policy facts: {battery:?}"
    );
}

#[test]
fn a_member_whose_class_is_unknown_defers_until_source_resolution() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig").unwrap();

    // The production default: nothing resolved yet, because no `.mo` has loaded.
    let network = read_network(&view, &root, &MemberClasses::default())
        .expect("waiting is not an error")
        .expect("the scope is still a network");
    assert!(
        network.pending_sources,
        "a network whose member classes are unread must report itself pending, so the projector \
         waits rather than compiling before the source declares its class"
    );
    assert!(
        network.components.is_empty(),
        "a pending network states no members: every conclusion drawn from a partial set — which \
         parts are unwired, which boundary outputs have sources — would be a false authoring error"
    );
}

#[test]
fn a_terminal_source_failure_is_reported_without_class_substitution() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig").unwrap();
    let mut classes = fixture_classes();
    classes.reject(
        "lunco://models/LunCo/Electrical/Battery.mo",
        "source failed to load",
    );

    let errors = read_network(&view, &root, &classes)
        .expect_err("a terminal source failure must prevent synthesis");
    assert!(errors.iter().any(|error| {
        error.path == "/Rig/Battery.info:sourceAsset" && error.message == "source failed to load"
    }));
}

#[test]
fn rejects_members_whose_opinions_cannot_be_generated() {
    let stage = stage("unusable_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig").unwrap();

    let errors =
        read_network(&view, &root, &fixture_classes()).expect_err("unusable authoring is an error");
    assert!(
        errors
            .iter()
            .any(|error| error.path == "/Rig/Battery.info:sourceAsset"
                && error.message.contains("must author a .mo info:sourceAsset")),
        "a member without a source asset must be rejected at the authored boundary: {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.path == "/Rig/Motor.inputs:demand"),
        "a value that cannot be spelled as a Modelica real is reported against the property \
         that carries it, not left to fail in the compiler: {errors:?}"
    );
}
