//! TDD for [`AutopilotBehaviorSpec`] — the source-spec mirror that lets the UI
//! read/visualize a vessel's patrol waypoints and round-trip a spec for
//! interactive editing (Ctrl+LMB append, right-click delete) via the existing
//! `SetAutopilotBehavior` command.

use lunco_autopilot::{AutopilotBehaviorSpec, BehaviorSpec};

#[test]
fn spec_round_trips_patrol_waypoints() {
    // Legacy bare-array waypoint shape (`[[x,y,z], ...]`) must still parse —
    // the `PatrolWaypoint` custom Deserialize accepts it as a no-action waypoint.
    let json = r#"{"kind":"patrol","waypoints":[[1.0,0.0,0.0],[2.0,0.0,0.0]],"speed":0.6,"radius":2.0,"dwell":0.0}"#;
    let spec = AutopilotBehaviorSpec::from_json(json).expect("patrol spec parses");
    let wps = spec.patrol_waypoints().expect("patrol exposes waypoints");
    assert_eq!(wps.len(), 2);
    assert_eq!(wps[0].pos, [1.0, 0.0, 0.0]);
    assert!(wps[0].on_arrival.is_empty(), "legacy bare-array → no actions");
    // Round-trip back to JSON.
    let out = spec.to_json().expect("serialize");
    let reparsed: BehaviorSpec = serde_json::from_str(&out).expect("reparse");
    assert!(matches!(reparsed, BehaviorSpec::Patrol { .. }));
}

#[test]
fn patrol_waypoint_with_arrival_action_parses() {
    // The new declarative shape: a waypoint carrying an on-arrival tool action.
    // This is the core-data home for "fire a tool at a patrol waypoint" — no
    // rhai tree-composition needed.
    let json = r#"{"kind":"patrol","waypoints":[
        {"pos":[10,0,0],"on_arrival":[{"kind":"run_tool","tool":"science::take_photo"}]},
        {"pos":[10,0,10]}
    ],"speed":0.6,"radius":2.0}"#;
    let spec = AutopilotBehaviorSpec::from_json(json).expect("patrol with actions parses");
    let wps = spec.patrol_waypoints().expect("patrol exposes waypoints");
    assert_eq!(wps.len(), 2);
    assert_eq!(wps[0].pos, [10.0, 0.0, 0.0]);
    assert_eq!(wps[0].on_arrival.len(), 1, "first waypoint fires one tool");
    assert!(wps[1].on_arrival.is_empty(), "second waypoint has no actions");
}

#[test]
fn non_patrol_spec_has_no_patrol_waypoints() {
    let json = r#"{"kind":"brake"}"#;
    let spec = AutopilotBehaviorSpec::from_json(json).expect("brake spec parses");
    assert!(spec.patrol_waypoints().is_none(), "non-patrol exposes no waypoints");
}

#[test]
fn bad_json_is_a_clean_error() {
    assert!(AutopilotBehaviorSpec::from_json("{not json").is_err());
    assert!(AutopilotBehaviorSpec::from_json(r#"{"kind":"nope"}"#).is_err());
}

#[test]
fn run_tool_spec_round_trips() {
    // `run_tool` is the leaf a patrol sequence uses to fire a tool call
    // ("take photo at waypoint N"). It must round-trip through JSON so the
    // rhai prelude / Ctrl+LMB authoring paths and `build_tree` all agree.
    let json = r#"{"kind":"run_tool","tool":"science::take_photo","args":"{}"}"#;
    let spec = AutopilotBehaviorSpec::from_json(json).expect("run_tool spec parses");
    let out = spec.to_json().expect("serialize");
    let reparsed: BehaviorSpec = serde_json::from_str(&out).expect("reparse");
    match reparsed {
        BehaviorSpec::RunTool { tool, args } => {
            assert_eq!(tool, "science::take_photo");
            assert_eq!(args, "{}");
        }
        other => panic!("expected RunTool, got {other:?}"),
    }
}

#[test]
fn run_tool_spec_defaults_empty_args() {
    // `args` is `#[serde(default)]` — omitted in JSON, it must parse to "".
    let json = r#"{"kind":"run_tool","tool":"ping"}"#;
    let spec = AutopilotBehaviorSpec::from_json(json).expect("parses without args");
    let out = spec.to_json().expect("serialize");
    let reparsed: BehaviorSpec = serde_json::from_str(&out).expect("reparse");
    match reparsed {
        BehaviorSpec::RunTool { args, .. } => assert_eq!(args, ""),
        other => panic!("expected RunTool, got {other:?}"),
    }
}