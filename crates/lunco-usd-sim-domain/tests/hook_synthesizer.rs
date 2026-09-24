//! An AUTHORED synthesizer: the graph is read in Rust, the Modelica is emitted
//! by a rhai policy.
//!
//! This is the split doc 54 §2 states — facts in Rust, rules in rhai — applied
//! to model synthesis. What a component graph BECOMES (which class stands in for
//! a part, whether a fuse is inserted, what a low-fidelity variant omits) is
//! policy, and policy that needs a rebuild to change is policy in the wrong
//! place.

use lunco_usd_sim_domain::MemberClasses;
use lunco_usd_sim_domain::network::read_network;
use lunco_usd_sim_domain::synthesis::{
    SynthContext, SynthOutcome, SynthesizerRegistry, network_facts, register_hook_synthesizer,
};
use openusd::sdf::Path as SdfPath;
use std::path::PathBuf;

const POLICY: &str = r#"
fn emit(net) {
    let src = "model " + net.model_name + "\n";
    for name in net.inputs {
        src += "  input Real " + name + ";\n";
    }
    for output in net.boundary_outputs { src += "  output Real " + output.name + ";\n"; }
    let unit = net.units[0];
    let first = net.components[0];
    let second = net.components[1];
    src += "  " + unit.name + " " + unit.instance + ";\n";
    src += "equation\nend " + net.model_name + ";\n\n";
    src += "model " + unit.name + "\n";
    for input in unit.inputs { src += "  input Real " + input + ";\n"; }
    for output in unit.outputs { src += "  output Real " + output + ";\n"; }
    src += "  " + first.class + " " + first.instance + ";\n";
    src += "  " + second.class + " " + second.instance + ";\n";
    src += "equation\nend " + unit.name + ";\n";
    #{
        source: src,
        units: net.units,
        layout: net.layout,
        source_roots: net.source_roots,
        member_output_aliases: [],
    }
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
    let src = "model " + net.model_name + "\n";
    for input in net.inputs { src += "  input Real " + input + ";\n"; }
    for output in net.boundary_outputs { src += "  output Real " + output.name + ";\n"; }
    let unit = net.units[0];
    let first = net.components[0];
    let second = net.components[1];
    let policy_instance = "policy_unit";
    src += "  " + unit.name + " " + policy_instance + ";\n";
    src += "equation\nend " + net.model_name + ";\n\n";
    src += "model " + unit.name + "\n";
    for input in unit.inputs { src += "  input Real " + input + ";\n"; }
    for output in unit.outputs { src += "  output Real " + output + ";\n"; }
    src += "  " + first.class + " " + first.instance + ";\n";
    src += "  " + second.class + " " + second.instance + ";\n";
    src += "equation\nend " + unit.name + ";\n\n";
    #{
        source: src,
        units: [#{
            name: unit.name,
            instance: policy_instance,
            components: unit.components,
            inputs: unit.inputs,
            outputs: unit.outputs,
        }],
        layout: layout,
        source_roots: net.source_roots,
        member_output_aliases: [],
    }
}
"#;

fn stage(fixture: &str) -> lunco_usd_bevy_stage::canonical::CanonicalStage {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    let composed =
        lunco_usd_bevy_stage::compose::compose_file_to_stage(&path).expect("compose fixture");
    lunco_usd_bevy_stage::canonical::CanonicalStage::from_stage(
        composed,
        path.to_string_lossy().to_string(),
    )
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
    let root = SdfPath::new("/Rig").unwrap();
    let classes = fixture_classes();
    let ctx = SynthContext { classes: &classes };

    let outcome = synthesizer
        .synthesize(&view, &root, "Rig_System", &ctx)
        .expect("the policy is not an authoring error");
    let SynthOutcome::Ready(synthesized) = outcome else {
        panic!("the fixture is a network and its classes resolve, so it must be Ready");
    };

    assert!(
        synthesized
            .source
            .contains("LunCo.Electrical.Battery Battery;"),
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
    let root = SdfPath::new("/Rig").unwrap();
    let classes = fixture_classes();
    let ctx = SynthContext { classes: &classes };

    let outcome = synthesizer
        .synthesize(&view, &root, "Rig_System", &ctx)
        .expect("the policy result is valid");
    let SynthOutcome::Ready(synthesized) = outcome else {
        panic!("the fixture is a network and its classes resolve");
    };

    assert_eq!(synthesized.units.len(), 1);
    assert_eq!(synthesized.units[0].instance, "policy_unit");
    assert_eq!(
        synthesized.units[0].component_paths,
        vec!["/Rig/Battery", "/Rig/Motor"]
    );
    assert_eq!(
        synthesized.layout.member_positions["/Rig/Battery"].0, -60,
        "the policy-owned placement is applied rather than recomputed"
    );
    assert!(synthesized.source.contains("model Rig_System"));
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
    let root = SdfPath::new("/Rig").unwrap();
    let classes = fixture_classes();
    let ctx = SynthContext { classes: &classes };

    let errors = synthesizer
        .synthesize(&view, &root, "Rig_System", &ctx)
        .expect_err("a policy that returns a number has emitted no model");
    assert!(
        errors[0]
            .message
            .contains("must return a map with a Modelica `source` key"),
        "the report has to name what the policy did wrong, not blame the scene: {errors:?}"
    );
}

#[test]
fn a_policy_must_return_the_complete_synthesis_schema() {
    let policy = POLICY.replace(
        "units: net.units,\n        layout: net.layout,\n        source_roots: net.source_roots,\n        member_output_aliases: [],",
        "member_output_aliases: [],",
    );
    lunco_hooks_rhai::register_rhai_hook("synth.incomplete-plan", "emit", &policy, true)
        .expect("policy compiles");

    let mut registry = SynthesizerRegistry::default();
    register_hook_synthesizer(&mut registry, "incomplete-plan");
    let synthesizer = registry.get("incomplete-plan").expect("registered").clone();
    let stage = stage("electrical_network.usda");
    let errors = synthesizer
        .synthesize(
            &stage.view(),
            &SdfPath::new("/Rig").unwrap(),
            "Rig_System",
            &SynthContext {
                classes: &fixture_classes(),
            },
        )
        .expect_err("omitted policy schema fields must be rejected");
    assert!(errors[0].message.contains("must return `units`"));
}

#[test]
fn a_policy_with_syntactically_valid_but_incomplete_source_is_rejected() {
    lunco_hooks_rhai::register_rhai_hook(
        "synth.invalid-source",
        "emit",
        r#"fn emit(net) { #{ source: "model " + net.model_name + "\nequation\nend " + net.model_name + ";\n" } }"#,
        true,
    )
    .expect("policy compiles");

    let mut registry = SynthesizerRegistry::default();
    register_hook_synthesizer(&mut registry, "invalid-source");
    let synthesizer = registry.get("invalid-source").expect("registered").clone();
    let stage = stage("electrical_network.usda");
    let root = SdfPath::new("/Rig").unwrap();
    let errors = synthesizer
        .synthesize(
            &stage.view(),
            &root,
            "Rig_System",
            &SynthContext {
                classes: &fixture_classes(),
            },
        )
        .expect_err("an empty wrapper must not be admitted as a generated network");
    assert!(
        errors[0].message.contains("must return `units`")
            || errors[0].message.contains("root boundary input")
            || errors[0].message.contains("generated unit")
    );
}

#[test]
fn a_policy_cannot_extend_the_authored_boundary_surface() {
    let policy = POLICY.replace(
        "let unit = net.units[0];",
        "src += \"  output Real invented;\\n\";\n    let unit = net.units[0];",
    );
    lunco_hooks_rhai::register_rhai_hook("synth.extra-port", "emit", &policy, true)
        .expect("policy compiles");

    let mut registry = SynthesizerRegistry::default();
    register_hook_synthesizer(&mut registry, "extra-port");
    let synthesizer = registry.get("extra-port").expect("registered").clone();
    let stage = stage("electrical_network.usda");
    let root = SdfPath::new("/Rig").unwrap();
    let errors = synthesizer
        .synthesize(
            &stage.view(),
            &root,
            "Rig_System",
            &SynthContext {
                classes: &fixture_classes(),
            },
        )
        .expect_err("a policy cannot invent a root boundary output");
    assert!(
        errors[0]
            .message
            .contains("root declares undeclared boundary output `invented`")
    );
}

#[test]
fn a_policy_cannot_promote_an_output_missing_from_the_loaded_class() {
    let policy = POLICY.replace(
        "member_output_aliases: [],",
        r#"member_output_aliases: [#{ member_path: "/Rig/Battery", output: "not_real", alias: "bad" }],"#,
    );
    lunco_hooks_rhai::register_rhai_hook("synth.bad-member-output", "emit", &policy, true)
        .expect("policy compiles");

    let mut registry = SynthesizerRegistry::default();
    register_hook_synthesizer(&mut registry, "bad-member-output");
    let synthesizer = registry
        .get("bad-member-output")
        .expect("registered")
        .clone();
    let stage = stage("electrical_network.usda");
    let root = SdfPath::new("/Rig").unwrap();
    let errors = synthesizer
        .synthesize(
            &stage.view(),
            &root,
            "Rig_System",
            &SynthContext {
                classes: &fixture_classes(),
            },
        )
        .expect_err("a policy cannot promote an undeclared Modelica output");
    assert!(errors[0].message.contains("not a declared member output"));
}

#[test]
fn a_policy_cannot_overlap_generated_member_layout_positions() {
    let policy = POLICY_WITH_PLAN.replace("x: member.x + 40, y: member.y", "x: 0, y: 0");
    lunco_hooks_rhai::register_rhai_hook("synth.overlap-layout", "emit", &policy, true)
        .expect("policy compiles");

    let mut registry = SynthesizerRegistry::default();
    register_hook_synthesizer(&mut registry, "overlap-layout");
    let synthesizer = registry.get("overlap-layout").expect("registered").clone();
    let stage = stage("electrical_network.usda");
    let root = SdfPath::new("/Rig").unwrap();
    let errors = synthesizer
        .synthesize(
            &stage.view(),
            &root,
            "Rig_System",
            &SynthContext {
                classes: &fixture_classes(),
            },
        )
        .expect_err("overlapping member nodes are not a usable generated diagram");
    assert!(errors[0].message.contains("places") && errors[0].message.contains("on top of"));
}

#[test]
fn facts_describe_the_whole_graph() {
    let stage = stage("electrical_network.usda");
    let view = stage.view();
    let root = SdfPath::new("/Rig").unwrap();
    let network = read_network(&view, &root, &fixture_classes())
        .expect("well-formed")
        .expect("a network");

    let facts = network_facts(&network, "Rig_System", Some(&fixture_classes()))
        .expect("network facts are valid");
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
