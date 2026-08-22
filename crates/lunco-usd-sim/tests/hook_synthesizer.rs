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
    SynthOutcome, SynthesizerRegistry, DEFAULT_SYNTHESIZER,
};
use openusd::sdf::Path as SdfPath;
use std::path::PathBuf;

const POLICY: &str = r#"
fn emit(net) {
    let src = "model " + net.model_name + "\n";
    for name in net.inputs {
        src += "  input Real " + name + ";\n";
    }
    for c in net.components {
        src += "  " + c.class + " " + c.instance + ";\n";
    }
    src += "equation\n";
    src += "end " + net.model_name + ";\n";
    #{ source: src }
}
"#;

const POLICY_WITH_PLAN: &str = r#"
fn emit(net) {
    let layout = #{ units: [], members: [] };
    for unit in net.layout.units {
        layout.units.push(#{ name: unit.name, x: unit.x + 25, y: unit.y });
    }
    for member in net.layout.members {
        layout.members.push(#{ path: member.path, x: member.x + 40, y: member.y });
    }
    #{
        source: "model " + net.model_name + "\nequation\nend " + net.model_name + ";\n",
        units: net.units,
        layout: layout,
    }
}
"#;

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
    let classes = fixture_classes();
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
    assert!(synthesized.inputs.contains("drive_left"));
    assert!(synthesized.outputs.contains("soc"));
}

#[test]
fn a_rhai_policy_can_replace_the_merge_partition_and_layout() {
    lunco_hooks_rhai::register_rhai_hook("synth.test-plan", "emit", POLICY_WITH_PLAN, true)
        .expect("policy compiles");

    let mut registry = SynthesizerRegistry::default();
    register_hook_synthesizer(&mut registry, "test-plan");
    let synthesizer = registry.get("test-plan").expect("registered").clone();

    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();
    let classes = fixture_classes();
    let ctx = SynthContext { classes: &classes };

    let outcome = synthesizer
        .synthesize(&view, &root, "Rig_Electrical_System", &ctx)
        .expect("the policy result is valid");
    let SynthOutcome::Ready(synthesized) = outcome else {
        panic!("the fixture is a network and its classes resolve");
    };

    assert_eq!(synthesized.units.len(), 1);
    assert_eq!(
        synthesized.units[0].component_paths,
        vec!["/Rig/Battery", "/Rig/Motor"]
    );
    assert_eq!(
        synthesized.layout.member_positions["/Rig/Battery"].0, -60,
        "the policy-owned placement is applied rather than recomputed"
    );
    assert!(synthesized.source.contains("model Rig_Electrical_System"));
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
    let classes = fixture_classes();
    let ctx = SynthContext { classes: &classes };

    let errors = synthesizer
        .synthesize(&view, &root, "Rig_Electrical_System", &ctx)
        .expect_err("a policy that returns a number has emitted no model");
    assert!(
        errors[0]
            .message
            .contains("must return a map with a Modelica `source` key"),
        "the report has to name what the policy did wrong, not blame the scene: {errors:?}"
    );
}

#[test]
fn facts_describe_the_whole_graph() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();
    let network = read_network(&view, &root, &fixture_classes())
        .expect("well-formed")
        .expect("a network");

    let facts = network_facts(&network, "Rig_Electrical_System", Some(&fixture_classes()));
    let components = facts.get("components").expect("components");
    let lunco_hooks::HookValue::Array(components) = components else {
        panic!("components is an array");
    };
    let battery = components
        .iter()
        .find(|c| c.get("path").and_then(|v| v.as_str()) == Some("/Rig/Battery"))
        .expect("battery is in the facts");
    assert_eq!(
        battery.get("class").and_then(|v| v.as_str()),
        Some("LunCo.Electrical.Battery")
    );
    assert!(
        battery.get("connectors").and_then(|v| v.get("p")).is_some(),
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

#[test]
fn shipped_default_policy_emits_visual_and_executable_topology() {
    let source = lunco_assets::scripting::policy("synth_acausal_network")
        .expect("the shipped synthesis policy is embedded");
    lunco_hooks_rhai::register_rhai_hook("synth.acausal-network", "synthesize", source, true)
        .expect("the shipped synthesis policy compiles");

    let registry = SynthesizerRegistry::default();
    let synthesizer = registry
        .get(DEFAULT_SYNTHESIZER)
        .expect("the default owner is the Rhai synthesizer")
        .clone();
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig/Electrical").unwrap();
    let classes = fixture_classes();
    let outcome = synthesizer
        .synthesize(
            &view,
            &root,
            "Rig_Electrical_System",
            &SynthContext { classes: &classes },
        )
        .expect("the shipped policy result is valid");
    let SynthOutcome::Ready(plan) = outcome else {
        panic!("the fixture must synthesize");
    };

    assert!(
        plan.source.contains("connect("),
        "the generated source has no topology"
    );
    assert!(plan.source.contains("COMPOSED TOPOLOGY"));
    assert!(
        plan.source.contains("Ellipse("),
        "ports are part of the visual schema"
    );
    assert!(plan.source.contains("LunCo.Electrical.Battery"));
    assert!(plan.source.contains("LunCo.Electrical.DCMotor"));
    let interface = lunco_modelica::ast_extract::parse_model_interface(
        &plan.source,
        "shipped-synthesis-policy.mo",
    );
    assert_eq!(
        interface.model_name.as_deref(),
        Some("Rig_Electrical_System")
    );
    let ast = rumoca_phase_parse::parse_to_ast(&plan.source, "shipped-synthesis-policy.mo")
        .expect("the Rhai-owned source and visual annotations remain valid Modelica");
    assert!(
        lunco_modelica::diagram::find_class_by_qualified_name(&ast, "Rig_Electrical_System")
            .and_then(|class| lunco_modelica::annotations::extract_icon(&class.annotation))
            .is_some(),
        "the policy must emit a discoverable root icon"
    );
}
