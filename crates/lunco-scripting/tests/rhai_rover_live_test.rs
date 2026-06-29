//! Live, headless end-to-end test of the rhai scripting stack (P1–P4).
//!
//! A real `ScriptedModel { language: Rhai }` runs a scenario against a live
//! `World`. We assert the scenario actually drove the simulation:
//!   - P2: `on_start`/`on_tick` ran on the host entity.
//!   - P1: `cmd("DriveRover", …)` dispatched by NAME through `ApiCommandEvent`
//!         → reflect dispatch → the real `DriveRover` observer fired, with the
//!         `target` gid resolved back to the host `Entity`.
//!   - P3: `world_pos`/`world_forward` reads fed the pure-rhai `nav_to`
//!         steering, and `emit(...)` produced a `TelemetryEvent`.
//!   - P4: the declarative `run_plan` executor advanced objectives and emitted
//!         `OBJECTIVE_COMPLETE` / `PLAN_COMPLETE`.
//!
//! Spy `#[on_command]` handlers stand in for the real mobility/physics stack
//! (which lives in other crates) — the bridge dispatches to whatever `DriveRover`
//! is registered, so recording its arguments proves the whole path end-to-end.

use bevy::prelude::*;
use lunco_api::executor::api_command_dispatcher;
use lunco_api::registry::ApiEntityRegistry;
use lunco_core::{
    on_command, register_commands, Ack, ActiveCommandId, Command, GlobalEntityId, OpId,
    TelemetryEvent,
};
use lunco_doc::{DocumentHost, DocumentId};
use lunco_scripting::doc::{ScriptDocument, ScriptLanguage, ScriptedModel};
use lunco_scripting::{LunCoScriptingPlugin, ScriptRegistry};

const ROVER_GID: u64 = 7777;

// ── Spy commands (stand-ins for the real lunco-mobility DriveRover/BrakeRover) ─

#[derive(Resource, Default)]
struct DriveLog(Vec<(f64, f64)>); // (forward, steer) per DriveRover

#[derive(Resource, Default)]
struct BrakeCount(u32);

#[derive(Resource, Default)]
struct EventLog(Vec<String>); // names of every emitted TelemetryEvent

#[Command]
struct DriveRover {
    #[authz_target]
    target: Entity,
    forward: f64,
    steer: f64,
    seq: u32,
    tick: u64,
}

#[on_command(DriveRover)]
fn on_drive(_t: On<DriveRover>, mut log: ResMut<DriveLog>) {
    log.0.push((cmd.forward, cmd.steer));
}

#[Command]
struct BrakeRover {
    #[authz_target]
    target: Entity,
}

#[on_command(BrakeRover)]
fn on_brake(_t: On<BrakeRover>, mut count: ResMut<BrakeCount>) {
    let _ = cmd;
    count.0 += 1;
}

// A result-reporting command that "spawns" something and reports the new gid
// back via `Ack.assigned` — stands in for any create command (AddComponent,
// spawn, name allocation) whose result a script needs.
const SPAWNED_GID: i64 = 4242;

#[Command(default)]
struct SpawnThing {}

#[on_command(SpawnThing)]
fn on_spawn(_t: On<SpawnThing>) -> Result<Ack, String> {
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "gid": SPAWNED_GID });
    Ok(ack)
}

// Records values fed back from a script — proves the script captured cmd() data
// and threaded it into a follow-up command (create-then-manipulate).
#[derive(Resource, Default)]
struct CapturedData(Vec<i64>);

#[Command(default)]
struct Report {
    value: i64,
}

#[on_command(Report)]
fn on_report(_t: On<Report>, mut cap: ResMut<CapturedData>) {
    cap.0.push(cmd.value);
}

register_commands!(on_drive, on_brake, on_spawn, on_report);

fn spy_events(t: On<TelemetryEvent>, mut log: ResMut<EventLog>) {
    log.0.push(t.event().name.clone());
}

/// Build a headless app wired with the scripting stack + the command-dispatch
/// path + spies (no entities yet).
fn build_app() -> App {
    let mut app = App::new();
    // AssetPlugin is required by the scripting plugin's source-asset registration
    // (MinimalPlugins doesn't bundle it).
    app.add_plugins((
        MinimalPlugins,
        bevy::log::LogPlugin::default(),
        AssetPlugin::default(),
        LunCoScriptingPlugin,
    ));

    // Command-dispatch path that rhai `cmd()` rides (ApiCommandEvent → reflect).
    app.init_resource::<ApiEntityRegistry>()
        .init_resource::<ActiveCommandId>()
        // RunScenario returns Result<Ack,_>; the #[on_command] macro records it here.
        .init_resource::<lunco_core::CommandResults>()
        .add_observer(api_command_dispatcher);

    // Spies + the test commands (register_all_commands registers types+observers).
    app.init_resource::<DriveLog>()
        .init_resource::<BrakeCount>()
        .init_resource::<EventLog>()
        .init_resource::<CapturedData>()
        .add_observer(spy_events);
    register_all_commands(&mut app);
    app
}

/// Spawn a rover at the origin facing -Z (Bevy forward), with NO scenario
/// attached. GlobalTransform is set explicitly — MinimalPlugins has no
/// TransformPlugin to propagate it. Maps gid → entity so cmd() target
/// resolution + world_pos lookups work.
fn spawn_rover(app: &mut App) -> Entity {
    let rover = app
        .world_mut()
        .spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 0.0)),
            GlobalEntityId::from_raw(ROVER_GID),
        ))
        .id();
    app.world_mut()
        .resource_mut::<ApiEntityRegistry>()
        .assign(rover, GlobalEntityId::from_raw(ROVER_GID));
    rover
}

/// Spawn a rover carrying `RoverVessel` (so `list_entities().type == "rover"` and
/// the selection toolkit / formation tool library can find it) at world x = `x`.
fn spawn_typed_rover(app: &mut App, gid: u64, x: f32) -> Entity {
    let e = app
        .world_mut()
        .spawn((
            Transform::from_xyz(x, 0.0, 0.0),
            GlobalTransform::from(Transform::from_xyz(x, 0.0, 0.0)),
            GlobalEntityId::from_raw(gid),
            lunco_core::RoverVessel,
        ))
        .id();
    app.world_mut()
        .resource_mut::<ApiEntityRegistry>()
        .assign(e, GlobalEntityId::from_raw(gid));
    e
}

/// Attach/replace a scenario on `target_gid` through the real RunScenario command
/// path (ApiCommandEvent → dispatch), as the API / MCP / UI would.
fn run_scenario(app: &mut App, target_gid: u64, source: &str, id: u64) {
    use lunco_api::executor::ApiCommandEvent;
    app.world_mut().trigger(ApiCommandEvent {
        command: "RunScenario".to_string(),
        params: serde_json::json!({ "target": target_gid, "source": source }),
        id,
    });
    app.world_mut().flush();
}

/// Full setup: app + rover with `source` attached directly as a ScriptDocument +
/// ScriptedModel (bypasses the RunScenario command — used to test the runtime).
fn setup(source: &str) -> (App, Entity) {
    let mut app = build_app();
    let rover = spawn_rover(&mut app);

    let doc_id = DocumentId::new(1);
    let doc = ScriptDocument::new(1, ScriptLanguage::Rhai, source.to_string());
    app.world_mut()
        .resource_mut::<ScriptRegistry>()
        .documents
        .insert(doc_id, DocumentHost::new(doc));
    app.world_mut().entity_mut(rover).insert(ScriptedModel {
        document_id: Some(1),
        language: Some(ScriptLanguage::Rhai),
        ..default()
    });

    (app, rover)
}

/// Run one FixedUpdate, then flush so the dispatcher's queued command-triggers
/// (DriveRover/BrakeRover) actually execute and reach the spies.
fn tick(app: &mut App) {
    app.world_mut().run_schedule(FixedUpdate);
    app.world_mut().flush();
}

#[test]
fn rhai_scenario_drives_real_rover() {
    // The shipped declarative mission — first waypoint is far from the origin,
    // so the rover should be commanded to drive forward toward it.
    let source = include_str!("../rhai/examples/mission_plan.rhai");
    let (mut app, _rover) = setup(source);

    tick(&mut app);

    let drives = &app.world().resource::<DriveLog>().0;
    assert!(
        !drives.is_empty(),
        "scenario on_tick should have issued a DriveRover command"
    );
    let (forward, steer) = drives[0];
    assert!(
        forward > 0.0,
        "rover should be driven forward toward the first waypoint, got forward={forward}"
    );
    assert!(
        steer.is_finite() && steer.abs() <= 1.0,
        "steer must be a finite, clamped value, got {steer}"
    );
}

#[test]
fn rhai_plan_arrives_brakes_and_emits() {
    // Single objective placed AT the rover's position → arrived immediately →
    // brake + OBJECTIVE_COMPLETE + PLAN_COMPLETE, no forward drive.
    let source = r#"
        const PLAN = [ [0.0, 0.0, 0.0] ];
        fn on_start(me) { this.idx = 0; }
        fn on_tick(me) {
            this.idx = run_plan(me, PLAN, this.idx, 1.0, 5.0);
        }
    "#;
    let (mut app, _rover) = setup(source);

    tick(&mut app);

    assert!(
        app.world().resource::<BrakeCount>().0 >= 1,
        "arriving on the only waypoint should brake the rover"
    );

    let events = &app.world().resource::<EventLog>().0;
    assert!(
        events.iter().any(|n| n == "OBJECTIVE_COMPLETE"),
        "reaching a waypoint should emit OBJECTIVE_COMPLETE; got {events:?}"
    );
    assert!(
        events.iter().any(|n| n == "PLAN_COMPLETE"),
        "finishing the plan should emit PLAN_COMPLETE; got {events:?}"
    );
    assert!(
        app.world().resource::<DriveLog>().0.is_empty(),
        "already-arrived rover should not be driven forward"
    );
}

#[test]
fn run_scenario_command_attaches_and_runs() {
    // The real scenario-loading path: fire RunScenario through the SAME
    // ApiCommandEvent dispatch the HTTP API / MCP use. It must attach a
    // ScriptedModel to the bare entity and the runtime must then drive it.
    use lunco_api::executor::ApiCommandEvent;

    let mut app = build_app();
    let rover = spawn_rover(&mut app); // bare — no scenario yet
    assert!(
        app.world().get::<ScriptedModel>(rover).is_none(),
        "rover starts with no scenario"
    );

    let src = include_str!("../rhai/examples/mission_plan.rhai");
    app.world_mut().trigger(ApiCommandEvent {
        command: "RunScenario".to_string(),
        params: serde_json::json!({ "target": ROVER_GID, "source": src }),
        id: 1,
    });
    app.world_mut().flush(); // apply the deferred ScriptedModel insert

    assert!(
        app.world().get::<ScriptedModel>(rover).is_some(),
        "RunScenario should have attached a ScriptedModel"
    );

    tick(&mut app);
    assert!(
        !app.world().resource::<DriveLog>().0.is_empty(),
        "attached scenario should drive the rover on the next tick"
    );

    // Hot-reload: re-running RunScenario bumps the doc generation in place.
    let gen_before = {
        let model = app.world().get::<ScriptedModel>(rover).unwrap();
        let id = model.document_id.unwrap();
        app.world()
            .resource::<ScriptRegistry>()
            .documents
            .get(&DocumentId::new(id))
            .unwrap()
            .document()
            .generation
    };
    app.world_mut().trigger(ApiCommandEvent {
        command: "RunScenario".to_string(),
        params: serde_json::json!({ "target": ROVER_GID, "source": "fn on_tick(me){}" }),
        id: 2,
    });
    app.world_mut().flush();
    let id = app
        .world()
        .get::<ScriptedModel>(rover)
        .unwrap()
        .document_id
        .unwrap();
    let gen_after = app
        .world()
        .resource::<ScriptRegistry>()
        .documents
        .get(&DocumentId::new(id))
        .unwrap()
        .document()
        .generation;
    assert_eq!(
        gen_after,
        gen_before + 1,
        "re-running RunScenario should hot-reload (bump generation) in place"
    );
}

#[test]
fn rhai_event_delivered_to_on_event_next_tick() {
    // P3 frame-delayed event delivery: tick 1 emits PING, tick 2 the on_event
    // hook receives it and records a marker via emit("GOT_PING").
    let source = r#"
        fn on_start(me) { this.sent = false; }
        fn on_tick(me) {
            if !this.sent { emit("PING", 1); this.sent = true; }
        }
        fn on_event(me, evt) {
            if evt.name == "PING" { emit("GOT_PING", evt.value); }
        }
    "#;
    let (mut app, _rover) = setup(source);

    tick(&mut app); // emits PING (collected into inbox)
    tick(&mut app); // inbox drained → on_event fires → emits GOT_PING

    let events = &app.world().resource::<EventLog>().0;
    assert!(
        events.iter().any(|n| n == "PING"),
        "tick 1 should emit PING; got {events:?}"
    );
    assert!(
        events.iter().any(|n| n == "GOT_PING"),
        "tick 2 on_event should have received PING and emitted GOT_PING; got {events:?}"
    );
}

#[test]
fn cmd_returns_data_enabling_create_then_manipulate() {
    // #5: cmd() must return the handler's assigned data, not just an id, so a
    // script can capture a spawned entity's gid and act on it in the SAME tick.
    // on_start spawns (reports gid 4242), then feeds that gid into Report — the
    // captured value proves the round-trip worked end-to-end through rhai.
    let source = r#"
        fn on_start(me) {
            let r = cmd("SpawnThing");
            // full-result form: r.ok / r.data
            if r.ok { cmd("Report", #{ value: r.data.gid }); }
            // convenience form: cmd_data returns the .data bag directly
            let d = cmd_data("SpawnThing", #{});
            cmd("Report", #{ value: d.gid });
        }
        fn on_tick(me) {}
    "#;
    let (mut app, _rover) = setup(source);

    tick(&mut app);

    let captured = &app.world().resource::<CapturedData>().0;
    assert_eq!(
        captured,
        &vec![SPAWNED_GID, SPAWNED_GID],
        "script should have captured the spawned gid from cmd() data (both the \
         full-result and cmd_data forms) and threaded it into Report; got {captured:?}"
    );
}

#[test]
fn registered_tool_library_callable_from_a_hook() {
    // #6/L3: a tool library registered via the RegisterToolLibrary command must
    // become a static module callable as `name::fn(...)` from inside on_tick,
    // and its functions must reach host verbs (cmd) across the module boundary.
    // Also exercises hot-reload: the runtime engine picks up the new lib on the
    // next tick.
    use lunco_api::executor::ApiCommandEvent;
    let mut app = build_app();
    let _rover = spawn_rover(&mut app);

    app.world_mut().trigger(ApiCommandEvent {
        command: "RegisterToolLibrary".to_string(),
        params: serde_json::json!({
            "name": "drivelib",
            "source": "fn drive_at(me, f) { cmd(\"DriveRover\", #{ target: me, forward: f, steer: 0.0, seq: 0, tick: 0 }); }",
        }),
        id: 1,
    });
    app.world_mut().flush();

    run_scenario(&mut app, ROVER_GID, "fn on_tick(me) { drivelib::drive_at(me, 0.7); }", 2);
    tick(&mut app);

    let drives = &app.world().resource::<DriveLog>().0;
    assert!(
        drives.iter().any(|(f, _)| (*f - 0.7).abs() < 1e-9),
        "drivelib::go should have driven the rover at forward=0.7; got {drives:?}"
    );
}

#[test]
fn builtin_formation_tool_library_drives_a_follower() {
    // The shipped `formation` tool library (formation::nearest_rover +
    // formation::hold_line) must work end-to-end: a follower scenario finds the
    // other rover via the prelude selection toolkit (called from inside the
    // library) and drives toward it.
    const LEADER_GID: u64 = 8001;
    let mut app = build_app();
    let _follower = spawn_typed_rover(&mut app, ROVER_GID, 0.0); // at origin
    let _leader = spawn_typed_rover(&mut app, LEADER_GID, 10.0); // 10 m ahead (+X)

    let src = r#"
        fn on_tick(me) {
            let leader = formation::nearest_rover(me);
            if leader != () { formation::hold_line(me, leader, 4.0, 1.0); }
        }
    "#;
    run_scenario(&mut app, ROVER_GID, src, 1);
    tick(&mut app);

    let drives = &app.world().resource::<DriveLog>().0;
    assert!(
        !drives.is_empty(),
        "formation tool library should have driven the follower toward the leader; got {drives:?}"
    );
}

/// Poll the `ScriptStatus` query for an entity (the unified diagnostics surface).
fn script_status(app: &mut App, gid: u64) -> serde_json::Value {
    use lunco_api::queries::ApiQueryRegistry;
    use lunco_api::schema::ApiResponse;
    let provider = app
        .world()
        .resource::<ApiQueryRegistry>()
        .get("ScriptStatus")
        .expect("ScriptStatus provider registered");
    match provider.execute(app.world_mut(), &serde_json::json!({ "target": gid })) {
        ApiResponse::Ok { data, .. } => data.expect("ScriptStatus returns data"),
        other => panic!("ScriptStatus returned {other:?}"),
    }
}

#[test]
fn script_status_reports_compile_error_then_clears_on_fix() {
    // Error feedback end-to-end on the UNIFIED store: a syntax error surfaces
    // through ScriptStatus (state=error + a located diagnostic), and a hot-reload
    // with valid source clears it back to ready.
    let mut app = build_app();
    let _rover = spawn_rover(&mut app);

    // 1. A scenario that fails to compile (empty RHS).
    run_scenario(&mut app, ROVER_GID, "fn on_tick(me) { let x = ; }", 1);
    tick(&mut app);
    let s = script_status(&mut app, ROVER_GID);
    assert_eq!(s["state"], "error", "compile error should be reported; got {s}");
    assert_eq!(s["ok"], false);
    let diags = s["diagnostics"].as_array().expect("diagnostics array");
    assert!(!diags.is_empty(), "expected a diagnostic; got {s}");
    assert!(
        !diags[0]["message"].as_str().unwrap_or("").is_empty(),
        "diagnostic should carry a message; got {s}"
    );
    assert!(
        diags[0]["line"].is_number(),
        "diagnostic should carry a 1-based line; got {s}"
    );

    // 2. Hot-reload with valid source → status clears to ready.
    run_scenario(&mut app, ROVER_GID, "fn on_tick(me) { }", 2);
    tick(&mut app);
    let s2 = script_status(&mut app, ROVER_GID);
    assert_eq!(s2["state"], "ready", "fix should clear the error; got {s2}");
    assert_eq!(s2["ok"], true);
    assert!(s2["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn script_status_reports_runtime_hook_errors() {
    // A scenario that COMPILES but throws at runtime (indexing past an array)
    // must also surface as an error through the same store.
    let mut app = build_app();
    let _rover = spawn_rover(&mut app);

    run_scenario(
        &mut app,
        ROVER_GID,
        "fn on_tick(me) { let a = [1]; let b = a[5]; }",
        1,
    );
    tick(&mut app);

    let s = script_status(&mut app, ROVER_GID);
    assert_eq!(
        s["state"], "error",
        "runtime hook failure should be reported; got {s}"
    );
    assert!(!s["diagnostics"].as_array().unwrap().is_empty());
}

/// Poll the `ScriptInspect` query — the live runtime-state surface.
fn script_inspect(app: &mut App, gid: u64) -> serde_json::Value {
    use lunco_api::queries::ApiQueryRegistry;
    use lunco_api::schema::ApiResponse;
    let provider = app
        .world()
        .resource::<ApiQueryRegistry>()
        .get("ScriptInspect")
        .expect("ScriptInspect provider registered");
    match provider.execute(app.world_mut(), &serde_json::json!({ "target": gid })) {
        ApiResponse::Ok { data, .. } => data.expect("ScriptInspect returns data"),
        other => panic!("ScriptInspect returned {other:?}"),
    }
}

#[test]
fn script_inspect_reports_live_state_hooks_and_health() {
    // Runtime introspection end-to-end: a scenario that accumulates state in
    // `this` is readable LIVE through ScriptInspect — its per-entity state
    // object, the hooks it defines, the started/running flags, and a healthy
    // status block, all in one call.
    let mut app = build_app();
    let _rover = spawn_rover(&mut app);

    run_scenario(
        &mut app,
        ROVER_GID,
        "fn on_start(me) { this.count = 0; } fn on_tick(me) { this.count += 1; }",
        1,
    );
    tick(&mut app); // on_start + first on_tick
    tick(&mut app); // second on_tick

    let s = script_inspect(&mut app, ROVER_GID);
    assert_eq!(s["scripted"], true, "rover has a scenario; got {s}");
    assert_eq!(s["running"], true, "compiled+started+unpaused; got {s}");
    assert_eq!(s["compiled"], true, "got {s}");
    assert_eq!(s["started"], true, "got {s}");
    assert_eq!(s["language"], "Rhai", "got {s}");
    assert_eq!(s["paused"], false, "got {s}");

    // Live `this` state is visible — count advanced with the ticks.
    assert_eq!(
        s["state"]["count"].as_i64(),
        Some(2),
        "live this.count should reflect the ticks; got {s}"
    );

    // The hooks the program defines are reported (and only those).
    let names: Vec<&str> = s["hooks"]
        .as_array()
        .expect("hooks array")
        .iter()
        .filter_map(|h| h.as_str())
        .collect();
    assert!(names.contains(&"on_start"), "got {names:?}");
    assert!(names.contains(&"on_tick"), "got {names:?}");
    assert!(!names.contains(&"on_event"), "on_event undefined; got {names:?}");

    // The unified health block rides along, ready (no errors).
    assert_eq!(s["status"]["state"], "ready", "got {s}");
}

#[test]
fn script_inspect_reports_unscripted_entity() {
    // A bare entity with no scenario returns scripted:false — not an error.
    let mut app = build_app();
    let _rover = spawn_rover(&mut app);
    let s = script_inspect(&mut app, ROVER_GID);
    assert_eq!(s["scripted"], false, "got {s}");
}

#[test]
fn run_timeline_lowers_data_to_a_running_scenario() {
    // Layer 2 end-to-end: fire RunTimeline with a pure-DATA timeline over the
    // SAME ApiCommandEvent path the API/MCP use. The handler must serialise it
    // into the generic executor, attach a ScriptedModel, and the runtime must
    // drive the rover from the first `move_to` step (a far waypoint → drive forward).
    use lunco_api::executor::ApiCommandEvent;

    let mut app = build_app();
    let rover = spawn_rover(&mut app); // bare, at origin facing -Z
    assert!(app.world().get::<ScriptedModel>(rover).is_none());

    // Object form with a far waypoint, then a brake command step.
    let timeline = serde_json::json!({
        "name": "t",
        "steps": [
            { "move_to": [0.0, 0.0, -20.0], "speed": 1.0, "radius": 2.0 },
            { "cmd": "BrakeRover", "params": {} },
        ],
    })
    .to_string();

    app.world_mut().trigger(ApiCommandEvent {
        command: "RunTimeline".to_string(),
        params: serde_json::json!({ "target": ROVER_GID, "timeline": timeline }),
        id: 1,
    });
    app.world_mut().flush();

    assert!(
        app.world().get::<ScriptedModel>(rover).is_some(),
        "RunTimeline should attach a ScriptedModel"
    );

    tick(&mut app);

    let drives = &app.world().resource::<DriveLog>().0;
    assert!(
        !drives.is_empty() && drives[0].0 > 0.0,
        "first move_to step should drive the rover forward toward the waypoint; got {drives:?}"
    );
}

#[test]
fn run_timeline_arrives_advances_and_brakes() {
    // A move_to step placed AT the rover (large radius) arrives immediately: nav_to
    // brakes, the step completes (STEP_COMPLETE), and the next `cmd` step fires
    // its BrakeRover. Proves data-step lowering + advancement + the cmd step.
    use lunco_api::executor::ApiCommandEvent;

    let mut app = build_app();
    let _rover = spawn_rover(&mut app);

    let timeline = serde_json::json!([
        { "move_to": [0.0, 0.0, 0.0], "radius": 5.0 },
        { "emit": "ARRIVED_A", "value": true },
    ])
    .to_string();

    app.world_mut().trigger(ApiCommandEvent {
        command: "RunTimeline".to_string(),
        params: serde_json::json!({ "target": ROVER_GID, "timeline": timeline }),
        id: 1,
    });
    app.world_mut().flush();

    tick(&mut app);

    assert!(
        app.world().resource::<BrakeCount>().0 >= 1,
        "arriving on the move_to step should brake"
    );
    let events = &app.world().resource::<EventLog>().0;
    assert!(
        events.iter().any(|n| n == "STEP_COMPLETE"),
        "completing the move_to step should emit STEP_COMPLETE; got {events:?}"
    );
}

#[test]
fn timeline_storage_register_discover_and_run() {
    // Timeline persistence/discovery end-to-end: RegisterTimeline stores a named
    // mission; ListTimelines/GetTimeline surface it; RunStoredTimeline runs it by
    // name and the runtime drives the rover from its first move_to step. (No Twin
    // in the test harness → the store is in-memory; file persistence is unit-tested
    // in timelines.rs.)
    use lunco_api::executor::ApiCommandEvent;
    use lunco_api::queries::ApiQueryRegistry;
    use lunco_api::schema::ApiResponse;

    let mut app = build_app();
    let rover = spawn_rover(&mut app);

    let timeline = serde_json::json!({
        "steps": [ { "move_to": [0.0, 0.0, -20.0], "speed": 1.0, "radius": 2.0 } ]
    })
    .to_string();

    // 1. Register a named timeline.
    app.world_mut().trigger(ApiCommandEvent {
        command: "RegisterTimeline".to_string(),
        params: serde_json::json!({ "name": "approach", "timeline": timeline }),
        id: 1,
    });
    app.world_mut().flush();

    // 2. Discoverable via ListTimelines + GetTimeline.
    let provider = app
        .world()
        .resource::<ApiQueryRegistry>()
        .get("ListTimelines")
        .expect("ListTimelines registered");
    let list = match provider.execute(app.world_mut(), &serde_json::json!({})) {
        ApiResponse::Ok { data, .. } => data.expect("ListTimelines data"),
        other => panic!("ListTimelines returned {other:?}"),
    };
    let names: Vec<&str> = list["timelines"]
        .as_array()
        .expect("timelines array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(names.contains(&"approach"), "ListTimelines should include it; got {list}");

    let provider = app
        .world()
        .resource::<ApiQueryRegistry>()
        .get("GetTimeline")
        .expect("GetTimeline registered");
    let got = match provider.execute(app.world_mut(), &serde_json::json!({ "name": "approach" })) {
        ApiResponse::Ok { data, .. } => data.expect("GetTimeline data"),
        other => panic!("GetTimeline returned {other:?}"),
    };
    assert_eq!(got["name"], "approach", "got {got}");
    assert!(
        got["timeline"].as_str().unwrap_or("").contains("move_to"),
        "GetTimeline should return the stored JSON; got {got}"
    );

    // 3. Run it by name → the rover drives forward toward the waypoint.
    assert!(app.world().get::<ScriptedModel>(rover).is_none());
    app.world_mut().trigger(ApiCommandEvent {
        command: "RunStoredTimeline".to_string(),
        params: serde_json::json!({ "target": ROVER_GID, "name": "approach" }),
        id: 2,
    });
    app.world_mut().flush();
    assert!(
        app.world().get::<ScriptedModel>(rover).is_some(),
        "RunStoredTimeline should attach a ScriptedModel"
    );

    tick(&mut app);
    let drives = &app.world().resource::<DriveLog>().0;
    assert!(
        !drives.is_empty() && drives[0].0 > 0.0,
        "stored timeline should drive the rover forward; got {drives:?}"
    );
}

#[test]
fn run_stored_timeline_unknown_name_errors() {
    // Running a timeline that was never registered must not attach a scenario.
    use lunco_api::executor::ApiCommandEvent;
    let mut app = build_app();
    let rover = spawn_rover(&mut app);
    app.world_mut().trigger(ApiCommandEvent {
        command: "RunStoredTimeline".to_string(),
        params: serde_json::json!({ "target": ROVER_GID, "name": "nope" }),
        id: 1,
    });
    app.world_mut().flush();
    assert!(
        app.world().get::<ScriptedModel>(rover).is_none(),
        "an unknown stored timeline must not attach a scenario"
    );
}

// ── Networking: scripts are host-authoritative (client-gated) ────────────────

/// A networked `Client` must NOT run scripts — scripted behavior reaches it via
/// replication of the resulting entity state, not local re-execution (which would
/// double-fire `cmd()`/`emit()` into the client world and advance a per-entity
/// `this` that lives outside the replicated/reconciled set, diverging freely).
/// `Host` and single-player (`Standalone` / absent role) run normally.
#[test]
fn client_role_gates_script_execution() {
    use lunco_core::NetworkRole;
    // The shipped mission drives toward its first (far) waypoint on tick 1 — a
    // reliable "did the script run?" probe (cf. rhai_scenario_drives_real_rover).
    let src = include_str!("../rhai/examples/mission_plan.rhai");

    // Client → gated off: on_tick never issues a DriveRover.
    let (mut app, _r) = setup(src);
    app.world_mut().insert_resource(NetworkRole::Client);
    tick(&mut app);
    assert!(
        app.world().resource::<DriveLog>().0.is_empty(),
        "a networked Client must not run scripts"
    );

    // Host → runs.
    let (mut app2, _r2) = setup(src);
    app2.world_mut().insert_resource(NetworkRole::Host);
    tick(&mut app2);
    assert!(
        !app2.world().resource::<DriveLog>().0.is_empty(),
        "a Host must run scripts"
    );

    // Standalone (the default single-player role) → runs.
    let (mut app3, _r3) = setup(src);
    app3.world_mut().insert_resource(NetworkRole::Standalone);
    tick(&mut app3);
    assert!(
        !app3.world().resource::<DriveLog>().0.is_empty(),
        "Standalone must run scripts"
    );
}

// ── Lifecycle completeness: on_stop teardown + pause/resume ──────────────────

/// `on_stop` runs when the scripted entity despawns (the teardown prune in
/// `tick_rhai_models`).
#[test]
fn on_stop_fires_on_despawn() {
    let source = r#"
        fn on_start(self) { emit("STARTED"); }
        fn on_stop(self)  { emit("STOPPED"); }
        fn on_tick(self)  { }
    "#;
    let (mut app, rover) = setup(source);
    tick(&mut app); // on_start
    assert!(
        app.world().resource::<EventLog>().0.iter().any(|e| e == "STARTED"),
        "on_start should run first"
    );

    // Despawn → next runtime tick has no live state for it → on_stop + prune.
    app.world_mut().entity_mut(rover).despawn();
    tick(&mut app);

    let events = &app.world().resource::<EventLog>().0;
    assert!(
        events.iter().any(|e| e == "STOPPED"),
        "on_stop should fire when the scripted entity despawns; got {events:?}"
    );
}

/// Hot-reload (source/generation bump) tears down the OLD compile with `on_stop`
/// before starting the new one.
#[test]
fn on_stop_fires_on_hot_reload() {
    let v1 = r#"
        fn on_start(self) { emit("START_V1"); }
        fn on_stop(self)  { emit("STOP_V1"); }
        fn on_tick(self)  { }
    "#;
    let (mut app, _rover) = setup(v1);
    tick(&mut app); // start v1

    // Bump the document (new source + generation) → hot-reload on next tick.
    {
        use lunco_scripting::doc::ScriptOp;
        let mut reg = app.world_mut().resource_mut::<ScriptRegistry>();
        let host = reg.documents.get_mut(&DocumentId::new(1)).unwrap();
        host.apply(ScriptOp::SetSource(
            r#"fn on_start(self){emit("START_V2");} fn on_stop(self){} fn on_tick(self){}"#
                .to_string(),
        ))
        .expect("hot-reload edit applies");
    }
    tick(&mut app); // recompile: on_stop(v1) then on_start(v2)

    let events = &app.world().resource::<EventLog>().0;
    assert!(
        events.iter().any(|e| e == "STOP_V1"),
        "hot-reload should run the outgoing version's on_stop; got {events:?}"
    );
    assert!(
        events.iter().any(|e| e == "START_V2"),
        "the new version should start after teardown; got {events:?}"
    );
}

/// `SetScenarioPaused` halts `on_tick`; clearing it resumes.
#[test]
fn set_scenario_paused_halts_and_resumes_on_tick() {
    use lunco_api::executor::ApiCommandEvent;

    let source = r#"
        fn on_start(self) { }
        fn on_tick(self)  { emit("TICK"); }
    "#;
    let (mut app, _rover) = setup(source);
    tick(&mut app);
    let baseline = app.world().resource::<EventLog>().0.iter().filter(|e| *e == "TICK").count();
    assert!(baseline >= 1, "on_tick should run before pausing");

    // Pause via the command API.
    let set_paused = |app: &mut App, paused: bool| {
        app.world_mut().trigger(ApiCommandEvent {
            command: "SetScenarioPaused".to_string(),
            params: serde_json::json!({ "target": ROVER_GID, "paused": paused }),
            id: 0,
        });
        app.world_mut().flush();
    };

    set_paused(&mut app, true);
    tick(&mut app);
    tick(&mut app);
    let paused = app.world().resource::<EventLog>().0.iter().filter(|e| *e == "TICK").count();
    assert_eq!(paused, baseline, "paused scenario must not run on_tick");

    // Resume → on_tick fires again.
    set_paused(&mut app, false);
    tick(&mut app);
    let resumed = app.world().resource::<EventLog>().0.iter().filter(|e| *e == "TICK").count();
    assert!(resumed > paused, "resuming should run on_tick again");
}

// ── Scenario parameters: one source, reusable via `params` ──────────────────

/// `RunScenario` with a `params` JSON object exposes it to the script as the
/// `params` constant, readable in hooks (so the same source serves many
/// entities). Also exercises the command deserialization WITH the new field.
#[test]
fn scenario_params_readable_in_hooks() {
    use lunco_api::executor::ApiCommandEvent;

    let mut app = build_app();
    let _rover = spawn_rover(&mut app); // ROVER_GID
    let source = r#"fn on_start(self) { emit("TAG:" + params.tag); }"#;
    app.world_mut().trigger(ApiCommandEvent {
        command: "RunScenario".to_string(),
        params: serde_json::json!({
            "target": ROVER_GID,
            "source": source,
            "params": r#"{"tag":"hello"}"#,
        }),
        id: 1,
    });
    app.world_mut().flush();
    tick(&mut app);

    let events = &app.world().resource::<EventLog>().0;
    assert!(
        events.iter().any(|e| e == "TAG:hello"),
        "script should read params.tag from the injected params constant; got {events:?}"
    );
}

// ── USD-embedded scenarios: scene-authored scenarios run on spawn ───────────

/// An entity stamped with `EmbeddedScenarioSource` (what the USD loader does for
/// a prim carrying `lunco:script`) is attached as a running scenario, and the
/// marker is consumed.
#[test]
fn usd_embedded_scenario_attaches_and_runs() {
    let mut app = build_app();
    let rover = spawn_rover(&mut app);
    // Simulate lunco-usd-bevy reading `lunco:script` off the prim.
    app.world_mut().entity_mut(rover).insert(lunco_core::EmbeddedScenarioSource(
        r#"fn on_start(self) { emit("EMBEDDED_RAN"); }"#.to_string(),
    ));

    tick(&mut app); // attach_embedded_scenarios → inserts ScriptedModel
    tick(&mut app); // tick_rhai_scenarios → compile + on_start

    let events = &app.world().resource::<EventLog>().0;
    assert!(
        events.iter().any(|e| e == "EMBEDDED_RAN"),
        "embedded scenario should attach and run on spawn; got {events:?}"
    );
    // Attached as a scenario; the marker was consumed.
    assert!(app.world().entity(rover).get::<ScriptedModel>().is_some());
    assert!(
        app.world().entity(rover).get::<lunco_core::EmbeddedScenarioSource>().is_none(),
        "marker should be removed after attach"
    );
}
