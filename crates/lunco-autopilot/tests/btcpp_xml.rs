//! BehaviorTree.CPP v4 XML codec: round-trip and interop coverage.
//!
//! The contract the codec must hold: `xml_to_value(value_to_xml(spec)) == spec`, in
//! CANONICAL form — numeric fields come back in the type the `BehaviorSpec` field has
//! (`times` whole, `seconds` fractional), so a hand-authored `"times": 3.0` normalises
//! to `3`. Everything else (arrays, empty arrays, strings that look like numbers,
//! escape characters, foreign BT.CPP elements and their subtrees) must survive
//! unchanged.

use lunco_autopilot::btcpp_xml::{value_to_xml, xml_to_value};
use lunco_autopilot::AutopilotBehavior;
use serde_json::{json, Value};

/// JSON → XML → JSON must return the input unchanged.
fn roundtrip(v: Value) {
    let xml = value_to_xml(&v).expect("to_xml");
    let back = xml_to_value(&xml).expect("from_xml");
    assert_eq!(v, back, "\nXML was:\n{xml}");
}

/// JSON → XML → JSON must return `want` (the canonical form of `v`).
fn roundtrip_to(v: Value, want: Value) {
    let xml = value_to_xml(&v).expect("to_xml");
    let back = xml_to_value(&xml).expect("from_xml");
    assert_eq!(want, back, "\nXML was:\n{xml}");
}

/// The imported JSON must be a *loadable* `BehaviorSpec`, not just any JSON.
fn assert_builds(v: &Value) {
    let json = serde_json::to_string(v).unwrap();
    AutopilotBehavior::from_json(&json).unwrap_or_else(|e| panic!("spec must build: {e}\n{json}"));
}

// ── Vectors and vector-of-vectors (the `patrol` break) ───────────────────────

#[test]
fn patrol_one_waypoint_keeps_its_nesting() {
    let v =
        json!({"kind":"patrol","waypoints":[[10.0,0.0,-5.0]],"speed":0.6,"radius":2.0,"dwell":0.0});
    roundtrip(v.clone());
    assert_builds(&xml_to_value(&value_to_xml(&v).unwrap()).unwrap());
}

#[test]
fn patrol_no_waypoints_stays_an_empty_array() {
    let v = json!({"kind":"patrol","waypoints":[],"speed":0.6,"radius":2.0,"dwell":0.0});
    roundtrip(v.clone());
    assert_builds(&xml_to_value(&value_to_xml(&v).unwrap()).unwrap());
}

#[test]
fn patrol_many_waypoints() {
    let v = json!({
        "kind":"patrol",
        "waypoints":[[10.0,0.0,-5.0],[0.0,0.0,12.5],[-3.0,1.0,0.0]],
        "speed":0.6,"radius":2.0,"dwell":1.5
    });
    roundtrip(v.clone());
    assert_builds(&xml_to_value(&value_to_xml(&v).unwrap()).unwrap());
}

// ── Integer-typed scalars ────────────────────────────────────────────────────

#[test]
fn repeat_with_float_times_survives_as_a_whole_number() {
    // Hand-authored / rhai JSON hits this: `3.0`, not `3`.
    roundtrip_to(
        json!({"kind":"repeat","times":3.0,"child":{"kind":"brake"}}),
        json!({"kind":"repeat","times":3,"child":{"kind":"brake"}}),
    );
}

#[test]
fn repeat_with_int_times_roundtrips() {
    roundtrip(json!({"kind":"repeat","times":3,"child":{"kind":"brake"}}));
}

#[test]
fn retry_with_float_times_survives() {
    roundtrip_to(
        json!({"kind":"retry","times":2.0,"child":{"kind":"brake"}}),
        json!({"kind":"retry","times":2,"child":{"kind":"brake"}}),
    );
}

#[test]
fn integer_seconds_canonicalises_to_float() {
    roundtrip_to(
        json!({"kind":"timeout","seconds":5,"child":{"kind":"hold"}}),
        json!({"kind":"timeout","seconds":5.0,"child":{"kind":"hold"}}),
    );
}

#[test]
fn sub_millisecond_timeout_is_not_rounded_away() {
    roundtrip(json!({"kind":"timeout","seconds":0.0004,"child":{"kind":"hold"}}));
}

#[test]
fn cooldown_roundtrips() {
    roundtrip(json!({"kind":"cooldown","seconds":2.5,"child":{"kind":"brake"}}));
}

// ── Structure ────────────────────────────────────────────────────────────────

#[test]
fn empty_control_keeps_its_empty_children() {
    roundtrip(json!({"kind":"sequence","children":[]}));
}

#[test]
fn decorator_without_a_child_is_rejected() {
    let err = value_to_xml(&json!({"kind":"invert"})).unwrap_err();
    assert!(err.contains("child"), "{err}");
}

#[test]
fn leaf_with_a_child_is_rejected() {
    let err = value_to_xml(&json!({"kind":"brake","child":{"kind":"hold"}})).unwrap_err();
    assert!(err.contains("leaf"), "{err}");
}

#[test]
fn imported_decorator_without_a_child_is_rejected() {
    let err = xml_to_value(
        r#"<root BTCPP_format="4"><BehaviorTree ID="Main"><Inverter/></BehaviorTree></root>"#,
    )
    .unwrap_err();
    assert!(err.contains("one child"), "{err}");
}

#[test]
fn all_control_and_decorator_kinds_roundtrip() {
    roundtrip(json!({
        "kind":"forever",
        "child":{
            "kind":"reactive_selector",
            "children":[
                {"kind":"reactive_sequence","children":[
                    {"kind":"invert","child":{"kind":"path_blocked","distance":6.0}},
                    {"kind":"force_success","child":{"kind":"steer_clear","speed":0.4}}
                ]},
                {"kind":"selector","children":[
                    {"kind":"force_failure","child":{"kind":"fail"}},
                    {"kind":"succeed"}
                ]},
                {"kind":"parallel","require":"one","children":[
                    {"kind":"timeout","seconds":5.0,"child":{"kind":"hold"}},
                    {"kind":"cooldown","seconds":1.0,"child":{"kind":"brake"}}
                ]},
                {"kind":"sequence","children":[
                    {"kind":"retry","times":3,"child":{"kind":"drive_to","target":[1.0,0.0,1.0],"speed":0.5,"radius":2.0}},
                    {"kind":"repeat","times":2,"child":{"kind":"cruise","throttle":0.2,"steer":0.0}}
                ]}
            ]
        }
    }));
}

#[test]
fn all_leaf_kinds_roundtrip_and_build() {
    let v = json!({"kind":"sequence","children":[
        {"kind":"drive_to","target":[10.0,0.0,0.0],"speed":0.6,"radius":2.0},
        {"kind":"patrol","waypoints":[[1.0,0.0,1.0]],"speed":0.6,"radius":2.0,"dwell":0.0},
        {"kind":"arrived","target":[0.0,0.0,0.0],"radius":3.0},
        {"kind":"wait","seconds":1.5},
        {"kind":"cruise","throttle":0.3,"steer":-0.2},
        {"kind":"brake"},
        {"kind":"face","target":[5.0,0.0,0.0],"tolerance":8.0},
        {"kind":"facing","target":[5.0,0.0,0.0],"tolerance":8.0},
        {"kind":"succeed"},
        {"kind":"fail"},
        {"kind":"hold"},
        {"kind":"follow","target":7,"speed":0.6,"radius":5.0},
        {"kind":"intercept","target":9,"speed":0.7,"radius":3.0,"lead":1.0},
        {"kind":"obstacle_ahead","distance":8.0,"cone":40.0},
        {"kind":"path_blocked","distance":6.0},
        {"kind":"steer_clear","speed":0.5}
    ]});
    roundtrip(v.clone());
    assert_builds(&xml_to_value(&value_to_xml(&v).unwrap()).unwrap());
}

// ── BT.CPP v4 wire conformance ───────────────────────────────────────────────

#[test]
fn succeed_and_fail_use_the_btcpp_elements() {
    let xml =
        value_to_xml(&json!({"kind":"selector","children":[{"kind":"succeed"},{"kind":"fail"}]}))
            .unwrap();
    assert!(xml.contains("<AlwaysSuccess/>"), "{xml}");
    assert!(xml.contains("<AlwaysFailure/>"), "{xml}");
}

#[test]
fn parallel_emits_success_count_not_require() {
    let one = value_to_xml(&json!({"kind":"parallel","require":"one","children":[]})).unwrap();
    assert!(one.contains(r#"success_count="1""#), "{one}");
    assert!(!one.contains("require="), "{one}");
    let all = value_to_xml(&json!({"kind":"parallel","require":"all","children":[]})).unwrap();
    assert!(all.contains(r#"success_count="-1""#), "{all}");
}

#[test]
fn parallel_reads_nav2_success_count() {
    let v = xml_to_value(
        r#"<root BTCPP_format="4"><BehaviorTree ID="Main">
             <Parallel success_count="1" failure_count="1"><Action ID="brake"/></Parallel>
           </BehaviorTree></root>"#,
    )
    .unwrap();
    assert_eq!(v["kind"], "parallel");
    assert_eq!(v["require"], "one");
}

#[test]
fn emits_btcpp_wrapper() {
    let xml = value_to_xml(&json!({"kind":"brake"})).unwrap();
    assert!(xml.contains("BTCPP_format=\"4\""));
    assert!(xml.contains("<BehaviorTree ID=\"MainTree\">"));
    assert!(xml.contains("ID=\"brake\""));
}

// ── Multi-tree files, <SubTree>, and <root> handling ─────────────────────────

#[test]
fn main_tree_to_execute_picks_the_right_tree_and_subtrees_expand() {
    // The shape every real Groot2 file with a <SubTree> has.
    let xml = r#"<root BTCPP_format="4" main_tree_to_execute="Main">
      <BehaviorTree ID="Helper">
        <Sequence>
          <Action ID="wait" seconds="2.0"/>
          <Action ID="brake"/>
        </Sequence>
      </BehaviorTree>
      <BehaviorTree ID="Main">
        <Sequence>
          <Action ID="drive_to" target="[10.0,0.0,0.0]" speed="0.6" radius="2.0"/>
          <SubTree ID="Helper"/>
        </Sequence>
      </BehaviorTree>
    </root>"#;
    let v = xml_to_value(xml).unwrap();
    assert_eq!(
        v,
        json!({"kind":"sequence","children":[
            {"kind":"drive_to","target":[10.0,0.0,0.0],"speed":0.6,"radius":2.0},
            {"kind":"sequence","children":[
                {"kind":"wait","seconds":2.0},
                {"kind":"brake"}
            ]}
        ]})
    );
    assert_builds(&v);
}

#[test]
fn multiple_trees_without_a_main_are_rejected() {
    let err = xml_to_value(
        r#"<root BTCPP_format="4">
             <BehaviorTree ID="A"><Action ID="brake"/></BehaviorTree>
             <BehaviorTree ID="B"><Action ID="hold"/></BehaviorTree>
           </root>"#,
    )
    .unwrap_err();
    assert!(err.contains("main_tree_to_execute"), "{err}");
}

#[test]
fn recursive_subtree_is_rejected() {
    let err = xml_to_value(
        r#"<root BTCPP_format="4" main_tree_to_execute="Main">
             <BehaviorTree ID="Main"><Inverter><SubTree ID="Main"/></Inverter></BehaviorTree>
           </root>"#,
    )
    .unwrap_err();
    assert!(err.contains("recursive"), "{err}");
}

#[test]
fn unknown_subtree_reference_is_rejected() {
    let err = xml_to_value(
        r#"<root BTCPP_format="4"><BehaviorTree ID="Main"><SubTree ID="Nope"/></BehaviorTree></root>"#,
    )
    .unwrap_err();
    assert!(err.contains("Nope"), "{err}");
}

#[test]
fn empty_root_is_rejected() {
    let err = xml_to_value(r#"<root BTCPP_format="4"/>"#).unwrap_err();
    assert!(err.contains("no behaviour-tree"), "{err}");
}

#[test]
fn empty_behavior_tree_is_rejected() {
    let err =
        xml_to_value(r#"<root BTCPP_format="4"><BehaviorTree ID="MainTree"/></root>"#).unwrap_err();
    assert!(err.contains("exactly one"), "{err}");
}

#[test]
fn malformed_xml_is_rejected() {
    assert!(xml_to_value("<root><Sequence></root>").is_err());
    assert!(xml_to_value("not xml at all").is_err());
    assert!(xml_to_value("").is_err());
}

// ── Foreign (non-BehaviorSpec) elements ──────────────────────────────────────

#[test]
fn foreign_decorator_keeps_its_subtree() {
    // <Delay> is a real BT.CPP element we have no spec kind for. Its child must NOT
    // be dropped, and the element name must come back.
    let xml = r#"<root BTCPP_format="4"><BehaviorTree ID="Main">
      <Delay delay_msec="500"><Action ID="brake"/></Delay>
    </BehaviorTree></root>"#;
    let v = xml_to_value(xml).unwrap();
    assert_eq!(
        v,
        json!({"kind":"delay","delay_msec":500,"children":[{"kind":"brake"}]})
    );
    // …and back out to XML unchanged.
    let out = value_to_xml(&v).unwrap();
    assert!(out.contains("<Delay"), "{out}");
    assert!(out.contains(r#"ID="brake""#), "{out}");
    assert_eq!(xml_to_value(&out).unwrap(), v);
}

#[test]
fn groot2_shaped_file_with_subtree_and_foreign_nodes() {
    let xml = r#"<root BTCPP_format="4" main_tree_to_execute="Patrol">
      <BehaviorTree ID="Recover">
        <KeepRunningUntilFailure>
          <Action ID="steer_clear" speed="0.3"/>
        </KeepRunningUntilFailure>
      </BehaviorTree>
      <BehaviorTree ID="Patrol">
        <ReactiveFallback>
          <Sequence>
            <Condition ID="path_blocked" distance="6.0"/>
            <SubTree ID="Recover"/>
          </Sequence>
          <IfThenElse>
            <Condition ID="arrived" target="[10.0,0.0,0.0]" radius="3.0"/>
            <Action ID="brake"/>
            <Action ID="drive_to" target="[10.0,0.0,0.0]" speed="0.6" radius="2.0"/>
          </IfThenElse>
        </ReactiveFallback>
      </BehaviorTree>
    </root>"#;
    let v = xml_to_value(xml).unwrap();
    assert_eq!(v["kind"], "reactive_selector");
    // The SubTree expanded in place, foreign elements kept their children.
    let recover = &v["children"][0]["children"][1];
    assert_eq!(recover["kind"], "keep_running_until_failure");
    assert_eq!(recover["children"][0]["kind"], "steer_clear");
    let ite = &v["children"][1];
    assert_eq!(ite["kind"], "if_then_else");
    assert_eq!(ite["children"].as_array().unwrap().len(), 3);
    // Foreign elements survive a trip back through XML.
    roundtrip(v);
}

#[test]
fn custom_action_id_stays_an_action() {
    let xml = r#"<root BTCPP_format="4"><BehaviorTree ID="Main">
      <Action ID="deploy_mast" angle="45.0"/>
    </BehaviorTree></root>"#;
    let v = xml_to_value(xml).unwrap();
    assert_eq!(v, json!({"kind":"deploy_mast","angle":45.0}));
    let out = value_to_xml(&v).unwrap();
    assert!(out.contains(r#"<Action ID="deploy_mast""#), "{out}");
}

// ── Attribute value fidelity ─────────────────────────────────────────────────

#[test]
fn strings_that_look_like_scalars_stay_strings() {
    let v = json!({
        "kind":"deploy_mast",
        "label":"42",
        "flag":"true",
        "note":"NaN",
        "sci":"1e5",
        "blackboard":"{goal}",
        "plain":"hello"
    });
    roundtrip(v);
}

#[test]
fn escape_characters_survive() {
    let v = json!({"kind":"log","text":"line one\nline\ttwo <&> \"q\" 'p'"});
    let xml = value_to_xml(&v).unwrap();
    // XML attribute-value normalisation would turn a literal newline/tab into a space,
    // so they must be numeric character references on the wire.
    assert!(xml.contains("&#10;") || xml.contains("\\n"), "{xml}");
    assert_eq!(xml_to_value(&xml).unwrap(), v);
}

#[test]
fn reserved_attribute_names_are_rejected() {
    let err = xml_to_value(
        r#"<root BTCPP_format="4"><BehaviorTree ID="Main"><Action ID="wait" kind="pwn"/></BehaviorTree></root>"#,
    )
    .unwrap_err();
    assert!(err.contains("kind"), "{err}");
}

// ── Depth ────────────────────────────────────────────────────────────────────

#[test]
fn absurdly_deep_import_is_rejected_not_overflowed() {
    let mut xml = String::from(r#"<root BTCPP_format="4"><BehaviorTree ID="Main">"#);
    for _ in 0..5_000 {
        xml.push_str("<Inverter>");
    }
    xml.push_str(r#"<Action ID="brake"/>"#);
    for _ in 0..5_000 {
        xml.push_str("</Inverter>");
    }
    xml.push_str("</BehaviorTree></root>");
    let err = xml_to_value(&xml).unwrap_err();
    assert!(err.contains("deep"), "{err}");
}

#[test]
fn absurdly_deep_export_is_rejected_not_overflowed() {
    // Only a little past the cap: a *serde_json* `Value` that deep cannot be built (or
    // dropped) in a test thread without overflowing on its own — which is exactly why
    // the cap exists on the import side, where the attacker picks the depth.
    let mut v = json!({"kind":"brake"});
    for _ in 0..500 {
        v = json!({"kind":"invert","child":v});
    }
    let err = value_to_xml(&v).unwrap_err();
    assert!(err.contains("deep"), "{err}");
}
