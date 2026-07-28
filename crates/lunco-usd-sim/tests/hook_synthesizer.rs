//! An AUTHORED synthesizer: the graph is read in Rust, the Modelica is emitted
//! by a rhai policy.
//!
//! This is the split doc 54 §2 states — facts in Rust, rules in rhai — applied
//! to model synthesis. What a component graph BECOMES (which class stands in for
//! a part, whether a fuse is inserted, what a low-fidelity variant omits) is
//! policy, and policy that needs a rebuild to change is policy in the wrong
//! place.

use lunco_usd_sim::domain_projection::{
    network_facts, read_network, register_hook_synthesizer, MemberClasses, SynthContext,
    SynthOutcome, SynthesizerRegistry,
};
use openusd::sdf::Path as SdfPath;
use std::path::PathBuf;

/// A whole synthesizer, authored. It receives the composed network as a map and
/// returns Modelica text — no USD reading, no Rust.
const POLICY: &str = r#"
fn emit(net) {
    let src = "model " + net.model_name + "\n";
    for name in net.inputs {
        src += "  input Real " + name + ";\n";
    }
    for c in net.components {
        // The POLICY decides what a part becomes: here every member is emitted
        // as its declared class, but a fidelity switch or a substitution table
        // would live exactly here.
        src += "  " + c.class + " " + c.instance + ";\n";
    }
    src += "equation\n";
    src += "end " + net.model_name + ";\n";
    src
}
"#;

fn stage(fixture: &str) -> lunco_usd_bevy::CanonicalStage {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    let composed = lunco_usd_bevy::compose_file_to_stage(&path).expect("compose fixture");
    lunco_usd_bevy::CanonicalStage::from_stage(composed, path.to_string_lossy().to_string())
}

#[test]
fn a_rhai_policy_can_be_the_synthesizer() {
    lunco_hooks_rhai::register_rhai_hook("synth.test-emit", "emit", POLICY, true)
        .expect("policy compiles");

    let mut registry = SynthesizerRegistry::default();
    register_hook_synthesizer(&mut registry, "test-emit");
    let synthesizer = registry
        .get("test-emit")
        .expect("an authored synthesizer is registered like any other")
        .clone();

    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();
    let classes = MemberClasses::path_derived_only();
    let ctx = SynthContext { classes: &classes };

    let outcome = synthesizer
        .synthesize(&view, &root, "Rig_Electrical_System", &ctx)
        .expect("the policy is not an authoring error");
    let SynthOutcome::Ready(synthesized) = outcome else {
        panic!("the fixture is a network and its classes resolve, so it must be Ready");
    };

    assert!(
        synthesized
            .source
            .contains("LunCo.Electrical.Battery Rig_x2f_Battery;"),
        "the authored emitter's output is what gets compiled:\n{}",
        synthesized.source
    );
    assert!(synthesized.source.contains("input Real drive_left;"));
    // The BOUNDARY is still Rust's answer: it is what the runtime holds the
    // compiled model to, so a policy cannot silence its own contract check.
    assert!(synthesized.inputs.contains("drive_left"));
    assert!(synthesized.outputs.contains("soc"));
}

#[test]
fn a_policy_that_returns_the_wrong_shape_is_an_authoring_error() {
    lunco_hooks_rhai::register_rhai_hook("synth.bad-emit", "emit", "fn emit(net) { 42 }", true)
        .expect("policy compiles");

    let mut registry = SynthesizerRegistry::default();
    register_hook_synthesizer(&mut registry, "bad-emit");
    let synthesizer = registry.get("bad-emit").expect("registered").clone();

    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();
    let classes = MemberClasses::path_derived_only();
    let ctx = SynthContext { classes: &classes };

    let errors = synthesizer
        .synthesize(&view, &root, "Rig_Electrical_System", &ctx)
        .expect_err("a policy that returns a number has emitted no model");
    assert!(
        errors[0].message.contains("must return the Modelica source"),
        "the report has to name what the policy did wrong, not blame the scene: {errors:?}"
    );
}

#[test]
fn facts_describe_the_whole_graph() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();
    let network = read_network(&view, &root, &MemberClasses::path_derived_only())
        .expect("well-formed")
        .expect("a network");

    let facts = network_facts(&network, "Rig_Electrical_System");
    let components = facts.get("components").expect("components");
    let lunco_hooks::HookValue::Array(components) = components else {
        panic!("components is an array");
    };
    // A policy must be able to answer "what is wired to what" WITHOUT reading
    // USD itself — a second reader is a second definition of the network.
    let battery = components
        .iter()
        .find(|c| c.get("path").and_then(|v| v.as_str()) == Some("/Rig/Battery"))
        .expect("battery is in the facts");
    assert_eq!(
        battery.get("class").and_then(|v| v.as_str()),
        Some("LunCo.Electrical.Battery")
    );
    assert!(
        battery
            .get("connectors")
            .and_then(|v| v.get("p"))
            .is_some(),
        "acausal edges reach the policy: {battery:?}"
    );
    assert_eq!(
        battery
            .get("constants")
            .and_then(|v| v.get("voltage_nom"))
            .and_then(|v| v.as_f64()),
        Some(24.0)
    );
}
