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
    #{ source: src, member_output_aliases: [] }
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
        member_output_aliases: [],
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

fn scripting_test_source(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scripting/tests")
        .join(name)
        .canonicalize()
        .and_then(std::fs::read_to_string)
        .unwrap_or_else(|error| panic!("read Rhai synthesis contract {name}: {error}"))
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
    assert_eq!(synthesized.units[0].instance, "policy_unit");
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
    let root = SdfPath::new("/Rig/Electrical").unwrap();
    let errors = synthesizer
        .synthesize(
            &stage.view(),
            &root,
            "Rig_Electrical_System",
            &SynthContext {
                classes: &fixture_classes(),
            },
        )
        .expect_err("an empty wrapper must not be admitted as a generated network");
    assert!(
        errors[0].message.contains("root boundary input")
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
    let root = SdfPath::new("/Rig/Electrical").unwrap();
    let errors = synthesizer
        .synthesize(
            &stage.view(),
            &root,
            "Rig_Electrical_System",
            &SynthContext {
                classes: &fixture_classes(),
            },
        )
        .expect_err("a policy cannot invent a root boundary output");
    assert!(errors[0]
        .message
        .contains("root declares undeclared boundary output `invented`"));
}

#[test]
fn a_policy_cannot_promote_an_output_missing_from_the_loaded_class() {
    let policy = POLICY.replace(
        "#{ source: src, member_output_aliases: [] }",
        r#"#{ source: src, member_output_aliases: [#{ member_path: "/Rig/Battery", output: "not_real", alias: "bad" }] }"#,
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
    let root = SdfPath::new("/Rig/Electrical").unwrap();
    let errors = synthesizer
        .synthesize(
            &stage.view(),
            &root,
            "Rig_Electrical_System",
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
    let root = SdfPath::new("/Rig/Electrical").unwrap();
    let errors = synthesizer
        .synthesize(
            &stage.view(),
            &root,
            "Rig_Electrical_System",
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
    assert!(
        !plan.source.contains("COMPOSED TOPOLOGY") && !plan.source.contains("\\n"),
        "generated visuals must be standard annotations, not a poster or literal escape text"
    );
    assert!(plan.source.contains("LunCo.Electrical.Battery"));
    assert!(plan.source.contains("LunCo.Electrical.DCMotor"));
    assert!(plan.source_roots.contains("LunCo"));
    assert!(
        plan.source.contains("Motor.demand = drive_left;"),
        "an authored external boundary source must become a Modelica equation:\n{}",
        plan.source
    );
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
            .and_then(|class| {
                assert!(
                    lunco_modelica::annotations::extract_icon(&class.annotation).is_some(),
                    "the generated root needs a compact standard Icon"
                );
                assert!(
                    lunco_modelica::annotations::extract_diagram(&class.annotation).is_some(),
                    "the generated root needs a standard Diagram"
                );
                let unit_name = &plan
                    .units
                    .first()
                    .expect("at least one generated unit")
                    .name;
                assert!(
                    class
                        .components
                        .values()
                        .any(|component| component.type_name.to_string() == *unit_name),
                    "the root Diagram must contain generated unit instances"
                );
                assert!(
                    class
                        .components
                        .values()
                        .filter(|component| component.type_name.to_string() == *unit_name)
                        .all(|component| {
                            lunco_modelica::annotations::extract_placement(&component.annotation)
                                .is_some()
                        }),
                    "root members must be placed generated units"
                );
                Some(())
            })
            .is_some(),
        "the policy must emit a discoverable root visual hierarchy"
    );
    let unit_name = &plan
        .units
        .first()
        .expect("at least one generated unit")
        .name;
    let unit = lunco_modelica::diagram::find_class_by_qualified_name(&ast, unit_name)
        .expect("the generated unit class is emitted");
    assert!(
        lunco_modelica::annotations::extract_icon(&unit.annotation).is_some()
            && lunco_modelica::annotations::extract_diagram(&unit.annotation).is_some(),
        "each generated unit needs standard Icon and Diagram annotations"
    );
    assert!(
        unit.components
            .values()
            .any(|component| { component.type_name.to_string() == "LunCo.Electrical.Battery" })
            && unit
                .components
                .values()
                .any(|component| { component.type_name.to_string() == "LunCo.Electrical.DCMotor" }),
        "the unit Diagram must contain the native LunCo member classes"
    );
    assert!(
        unit.components
            .values()
            .filter(|component| component.type_name.to_string().starts_with("LunCo."))
            .all(|component| {
                lunco_modelica::annotations::extract_placement(&component.annotation).is_some()
            }),
        "native members need placements so their authored icons can render"
    );
}

#[test]
fn shipped_acausal_policy_contract_runs_in_rhai() {
    let policy = lunco_assets::scripting::policy("synth_acausal_network")
        .expect("the shipped synthesis policy is embedded");
    let contract = scripting_test_source("test_generated_acausal_policy.rhai");
    lunco_hooks_rhai::register_rhai_hook(
        "test.synthesized-acausal-contract",
        "test_generated_acausal_policy",
        &format!("{policy}\n{contract}"),
        true,
    )
    .expect("the shipped policy and its Rhai contract compile");

    let stage = stage("electrical_network.usda");
    let network = read_network(
        &stage.view(),
        &SdfPath::new("/Rig/Electrical").unwrap(),
        &fixture_classes(),
    )
    .expect("fixture is readable")
    .expect("fixture is a network");
    let facts = network_facts(&network, "Rig_Electrical_System", Some(&fixture_classes()));
    let result = lunco_hooks::invoke("test.synthesized-acausal-contract", &[facts])
        .expect("Rhai contract hook is registered")
        .expect("the shipped policy satisfies its Rhai contract");
    assert_eq!(result, lunco_hooks::HookValue::Bool(true));
}

#[test]
fn shipped_actuator_policy_contract_runs_in_rhai() {
    let policy = lunco_assets::scripting::policy("synth_actuator_wrench")
        .expect("the shipped actuator policy is embedded");
    let contract = scripting_test_source("test_generated_actuator_policy.rhai");
    lunco_hooks_rhai::register_rhai_hook(
        "test.synthesized-actuator-contract",
        "test_generated_actuator_policy",
        &format!("{policy}\n{contract}"),
        true,
    )
    .expect("the shipped actuator policy and its Rhai contract compile");

    let facts = lunco_hooks::HookValue::Map(vec![
        (
            "model_name".into(),
            lunco_hooks::HookValue::str("AttitudeActuation"),
        ),
        (
            "root".into(),
            lunco_hooks::HookValue::str("/Lander/Actuation"),
        ),
        (
            "inputs".into(),
            lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::str("desired_torque_z")]),
        ),
        (
            "outputs".into(),
            lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::str("valve")]),
        ),
        (
            "actuator_paths".into(),
            lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::str("/Lander/Thruster")]),
        ),
        (
            "wrench_matrix".into(),
            lunco_hooks::HookValue::Array(
                (0..6)
                    .map(|row| {
                        lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::Float(
                            if row == 5 { 1.0 } else { 0.0 },
                        )])
                    })
                    .collect(),
            ),
        ),
        ("allocation_step".into(), lunco_hooks::HookValue::Float(0.1)),
        ("actuator_count".into(), lunco_hooks::HookValue::Int(1)),
    ]);
    let result = lunco_hooks::invoke("test.synthesized-actuator-contract", &[facts])
        .expect("Rhai actuator contract hook is registered")
        .expect("the shipped actuator policy satisfies its Rhai contract");
    assert_eq!(result, lunco_hooks::HookValue::Bool(true));
}
