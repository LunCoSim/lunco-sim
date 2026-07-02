//! Live, headless end-to-end test of the rhai scripting stack (P1–P4).
//!
//! A real `ScriptedModel { language: Rhai }` runs a scenario against a live
//! `World`. We assert the scenario actually drove the simulation:
//!   - P2: `on_start`/`on_tick` ran on the host entity.
//!   - P1: `cmd("SetPorts", …)` dispatched by NAME through `ApiCommandEvent`
//!         → reflect dispatch → the real `SetPorts` observer fired, with the
//!         `target` gid resolved back to the host `Entity`.
//!   - P3: `world_pos`/`world_forward` reads fed the pure-rhai `nav_to`
//!         steering, and `emit(...)` produced a `TelemetryEvent`.
//!   - P4: the declarative `run_plan` executor advanced objectives and emitted
//!         `OBJECTIVE_COMPLETE` / `PLAN_COMPLETE`.
//!
//! Spy `#[on_command]` handlers stand in for the real mobility/physics stack
//! (which lives in other crates) — the bridge dispatches to whatever `SetPorts`
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

// ── Spy command (stand-in for the real `lunco_cosim::SetPorts`) ────────────────
// Control is now the ONE generic `SetPorts` command (a batch of named port
// writes). The spy records a drive sample (throttle/steer) into `DriveLog` and
// counts full-brake writes into `BrakeCount`, so the same recording proves the
// whole `cmd("SetPorts", …)` dispatch path end-to-end.

#[derive(Resource, Default)]
struct DriveLog(Vec<(f64, f64)>); // (throttle, steer) per drive SetPorts

#[derive(Resource, Default)]
struct BrakeCount(u32);

#[derive(Resource, Default)]
struct EventLog(Vec<String>); // names of every emitted TelemetryEvent

#[Command]
struct SetPorts {
    #[authz_target]
    target: Entity,
    writes: Vec<(String, f64)>,
    #[serde(default)]
    seq: u32,
    #[serde(default)]
    tick: u64,
}

#[on_command(SetPorts)]
fn on_drive(_t: On<SetPorts>, mut log: ResMut<DriveLog>, mut brakes: ResMut<BrakeCount>) {
    let get = |name: &str| cmd.writes.iter().find(|(n, _)| n == name).map(|(_, v)| *v);
    if get("brake").is_some_and(|b| b > 0.5) {
        brakes.0 += 1;
    }
    // A throttle/steer write is a drive sample (mirrors the old `DriveRover` log).
    if cmd.writes.iter().any(|(n, _)| n == "throttle" || n == "steer") {
        log.0.push((get("throttle").unwrap_or(0.0), get("steer").unwrap_or(0.0)));
    }
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

register_commands!(on_drive, on_spawn, on_report);

// ── Reflect targets for the native get/set verbs ──────────────────────────────
// A component and a resource, both reflect-registered, exercise the symmetric
// `set`/`get` (component) and `set_setting`/`get_setting` (resource) verbs across
// scalar, vector, bool and string fields — the native read/write path, no JSON.

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Knob {
    gain: f64,
    dir: Vec3,
    armed: bool,
    label: String,
}

#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct SimConfig {
    speed: f64,
    steps: i64,
}

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

    // Reflect targets for the get/set verbs (component + resource).
    app.register_type::<Knob>()
        .register_type::<SimConfig>()
        .init_resource::<SimConfig>();
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

/// Spawn a rover carrying `FlightSoftware` (so `list_entities().type == "rover"` and
/// the selection toolkit / formation tool library can find it) at world x = `x`.
fn spawn_typed_rover(app: &mut App, gid: u64, x: f32) -> Entity {
    let e = app
        .world_mut()
        .spawn((
            Transform::from_xyz(x, 0.0, 0.0),
            GlobalTransform::from(Transform::from_xyz(x, 0.0, 0.0)),
            GlobalEntityId::from_raw(gid),
            lunco_fsw::FlightSoftware::default(),
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
/// (SetPorts) actually execute and reach the spies.
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
        "scenario on_tick should have issued a SetPorts drive command"
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
fn builtin_task_advances_with_no_on_tick() {
    // The built-in task: declare `this.task = seq([...])` in on_start and the
    // engine advances it every tick — NO on_tick hook. step0 emits A and
    // advances; step1 is a wait_until(true) that clears in one tick; step2 emits B.
    let source = r#"
        fn on_start(me) {
            this.task = seq([
                once(|m| emit("A", 1)),
                wait_until(|m| true),
                once(|m| emit("B", 2)),
            ]);
        }
    "#;
    let (mut app, _rover) = setup(source);

    tick(&mut app); // step0 → emit A, advance
    tick(&mut app); // step1 wait_until(true) → advance
    tick(&mut app); // step2 → emit B

    let events = &app.world().resource::<EventLog>().0;
    assert!(events.iter().any(|n| n == "A"), "task step0 should emit A; got {events:?}");
    assert!(
        events.iter().any(|n| n == "B"),
        "task should self-advance (no on_tick) and emit B; got {events:?}"
    );
}

#[test]
fn builtin_task_waits_for_event_with_no_on_event() {
    // A task's wait_for(name) step completes from a delivered event even though
    // the script defines NO on_event — the engine feeds events into the task.
    // on_tick only PRODUCES the event; receiving it is the built-in's job.
    let source = r#"
        fn on_start(me) {
            this.sent = false;
            this.task = seq([
                wait_for("GO"),
                once(|m| emit("DONE", 1)),
            ]);
        }
        fn on_tick(me) {
            if !this.sent { emit("GO", 1); this.sent = true; }
        }
    "#;
    let (mut app, _rover) = setup(source);

    tick(&mut app); // emits GO (into inbox); task still on wait_for
    assert!(
        !app.world().resource::<EventLog>().0.iter().any(|n| n == "DONE"),
        "task must hold on wait_for(GO) before the event arrives"
    );
    tick(&mut app); // GO delivered → task feed advances past wait_for
    tick(&mut app); // next step runs → emit DONE

    assert!(
        app.world().resource::<EventLog>().0.iter().any(|n| n == "DONE"),
        "task should advance past wait_for(GO) via the engine event-feed (no on_event) and emit DONE"
    );
}

/// Count how many times event `name` was emitted.
fn event_count(app: &App, name: &str) -> usize {
    app.world().resource::<EventLog>().0.iter().filter(|n| n.as_str() == name).count()
}
fn emitted(app: &App, name: &str) -> bool {
    app.world().resource::<EventLog>().0.iter().any(|n| n == name)
}

#[test]
fn builtin_task_fn_sugar_auto_inits_with_no_on_start() {
    // Sugar: the whole scenario is one `fn task(me)` — no on_start, no
    // `this.task = …`. The engine seeds it after Start and advances it each tick.
    let source = r#"
        fn task(me) {
            seq([ once(|m| emit("X", 1)), once(|m| emit("Y", 1)) ])
        }
    "#;
    let (mut app, _rover) = setup(source);
    tick(&mut app); // Start → __init_task seeds this.task; Tick → step0 emits X
    tick(&mut app); // step1 emits Y
    assert!(
        emitted(&app, "X") && emitted(&app, "Y"),
        "`fn task(me)` should auto-init and advance with no on_start; got {:?}",
        app.world().resource::<EventLog>().0
    );
}

#[test]
fn builtin_task_par_all_waits_for_every_branch() {
    // par_all is done only when ALL branches finish. Branch A finishes tick 1
    // (a once); branch B is a 2-step seq that finishes tick 2 → the whole task
    // completes on tick 2, not tick 1.
    let source = r#"
        fn on_start(me) {
            this.task = par_all([
                once(|m| emit("A", 1)),
                seq([ once(|m| emit("B1", 1)), once(|m| emit("B2", 1)) ]),
            ]);
        }
    "#;
    let (mut app, _rover) = setup(source);
    tick(&mut app);
    assert!(emitted(&app, "A") && emitted(&app, "B1"), "tick1 runs both branches' first step");
    assert!(!emitted(&app, "TASK_COMPLETE"), "par_all must wait for branch B's 2nd step");
    tick(&mut app);
    assert!(emitted(&app, "B2"), "branch B advances on tick2");
    assert!(emitted(&app, "TASK_COMPLETE"), "par_all completes once all branches finish");
}

#[test]
fn builtin_task_par_race_completes_on_first_branch() {
    // par_race is done as soon as ANY branch finishes. One branch never completes
    // (wait_until false); the other finishes immediately → the task completes.
    let source = r#"
        fn on_start(me) {
            this.task = par_race([
                wait_until(|m| false),
                once(|m| emit("WIN", 1)),
            ]);
        }
    "#;
    let (mut app, _rover) = setup(source);
    tick(&mut app);
    assert!(emitted(&app, "WIN"), "the finishing branch ran");
    assert!(emitted(&app, "TASK_COMPLETE"), "par_race completes on the first finished branch");
}

#[test]
fn builtin_task_repeat_runs_body_n_times() {
    // repeat(3, ...) runs its body to completion three times.
    let source = r#"
        fn on_start(me) { this.task = repeat(3, once(|m| emit("R", 1))); }
    "#;
    let (mut app, _rover) = setup(source);
    for _ in 0..4 { tick(&mut app); }
    assert_eq!(event_count(&app, "R"), 3, "repeat(3) should run the body exactly 3 times");
    assert!(emitted(&app, "TASK_COMPLETE"), "repeat completes after the last iteration");
}

#[test]
fn builtin_task_forever_never_completes() {
    // forever re-runs its body and never reports done.
    let source = r#"
        fn on_start(me) { this.task = forever(once(|m| emit("F", 1))); }
    "#;
    let (mut app, _rover) = setup(source);
    for _ in 0..3 { tick(&mut app); }
    assert_eq!(event_count(&app, "F"), 3, "forever runs the body every tick");
    assert!(!emitted(&app, "TASK_COMPLETE"), "forever must never complete");
}

#[test]
fn builtin_mission_completes_and_emits() {
    // A `fn mission(me)` is auto-run like `fn task`. One objective whose condition
    // holds completes immediately → OBJECTIVE_COMPLETE + its on_complete + the
    // one-shot MISSION_COMPLETE.
    let source = r#"
        fn mission(me) {
            [ objective("reach", #{ done: |m| true, on_complete: |m| emit("REACHED", 1) }) ]
        }
    "#;
    let (mut app, _rover) = setup(source);
    tick(&mut app);
    assert!(emitted(&app, "REACHED"), "on_complete should fire");
    assert!(emitted(&app, "MISSION_COMPLETE"), "a one-objective mission completes; got {:?}",
        app.world().resource::<EventLog>().0);
}

#[test]
fn builtin_mission_requires_gate_locked_objectives() {
    // `b` requires `a`; `a`'s condition never holds → `a` stays active, `b` never
    // unlocks → neither b's on_complete nor MISSION_COMPLETE fire.
    let source = r#"
        fn mission(me) {
            [
                objective("a", #{ done: |m| false }),
                objective("b", #{ requires: ["a"], done: |m| true,
                                  on_complete: |m| emit("B_DONE", 1) }),
            ]
        }
    "#;
    let (mut app, _rover) = setup(source);
    for _ in 0..3 { tick(&mut app); }
    assert!(!emitted(&app, "B_DONE"), "b must stay locked until its prerequisite a completes");
    assert!(!emitted(&app, "MISSION_COMPLETE"), "mission can't complete while a is unmet");
}

#[test]
fn builtin_mission_fails_on_fail_condition() {
    // An objective whose `fail` condition trips → OBJECTIVE_FAILED + MISSION_FAILED.
    let source = r#"
        fn mission(me) {
            [ objective("survive", #{ done: |m| false, fail: |m| true }) ]
        }
    "#;
    let (mut app, _rover) = setup(source);
    tick(&mut app);
    assert!(emitted(&app, "MISSION_FAILED"), "a failed objective fails the mission; got {:?}",
        app.world().resource::<EventLog>().0);
    assert!(!emitted(&app, "MISSION_COMPLETE"), "a failed mission must not also report complete");
}

#[test]
fn builtin_mission_dwell_holds_until_satisfied() {
    // dwell requires the condition to hold for N sim-seconds. The test harness has
    // no clock (elapsed_seconds()==0), so a 5s dwell never elapses → the objective
    // stays active and the mission does not complete.
    let source = r#"
        fn mission(me) {
            [ objective("hold", #{ done: |m| true, dwell: 5.0,
                                   on_complete: |m| emit("HELD", 1) }) ]
        }
    "#;
    let (mut app, _rover) = setup(source);
    for _ in 0..3 { tick(&mut app); }
    assert!(!emitted(&app, "HELD"), "dwell must hold the condition before completing");
    assert!(!emitted(&app, "MISSION_COMPLETE"), "mission waits on the dwelling objective");
}

#[test]
fn builtin_task_and_mission_run_together() {
    // A scenario can run a behaviour (task) AND track success (mission) at once.
    let source = r#"
        fn task(me) { seq([ once(|m| emit("ACTING", 1)) ]) }
        fn mission(me) { [ objective("win", #{ done: |m| true, on_complete: |m| emit("WON", 1) }) ] }
    "#;
    let (mut app, _rover) = setup(source);
    tick(&mut app);
    assert!(emitted(&app, "ACTING"), "the task ran");
    assert!(emitted(&app, "WON") && emitted(&app, "MISSION_COMPLETE"), "the mission resolved");
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
            "source": "fn drive_at(me, f) { cmd(\"SetPorts\", #{ target: me, writes: [[\"throttle\", f], [\"steer\", 0.0]], seq: 0, tick: 0 }); }",
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
            { "cmd": "SetPorts", "params": { "writes": [["brake", 1.0]] } },
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
    // its brake SetPorts. Proves data-step lowering + advancement + the cmd step.
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

    // Client → gated off: on_tick never issues a SetPorts drive.
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

// ── Native get/set verbs: tune any reflected field/setting from a scenario ───

/// `set(id, "Comp.field", value)` writes straight onto the live component across
/// scalar / vector / bool / string fields (native → reflect, no JSON), coercing
/// an int literal into an f64 field. The edit lands in ECS, readable in Rust.
#[test]
fn set_verb_writes_component_fields() {
    let source = r#"
        fn on_start(me) {
            set(me, "Knob.gain", 3);          // int literal → f64 field (coerced)
            set(me, "Knob.dir", [1.0, 2.0, 3.0]);
            set(me, "Knob.armed", true);
            set(me, "Knob.label", "go");
        }
    "#;
    let (mut app, rover) = setup(source);
    app.world_mut().entity_mut(rover).insert(Knob::default());

    tick(&mut app);

    let knob = app.world().entity(rover).get::<Knob>().expect("Knob present");
    assert_eq!(knob.gain, 3.0, "int literal should coerce into the f64 field");
    assert_eq!(knob.dir, Vec3::new(1.0, 2.0, 3.0), "array should become a Vec3");
    assert!(knob.armed, "bool field should be set");
    assert_eq!(knob.label, "go", "string field should be set");
}

/// `set_setting`/`get_setting` reach a global `Resource` — the resource twin of
/// `set`/`get`. The write lands in the resource, and the read returns the same
/// native value (round-tripped back through a command), proving both halves.
#[test]
fn setting_verbs_read_and_write_resources() {
    let source = r#"
        fn on_start(me) {
            set_setting("SimConfig.speed", 9.5);
            set_setting("SimConfig.steps", 7);
            // read it straight back natively and feed it to a command
            cmd("Report", #{ value: get_setting("SimConfig.steps") });
        }
    "#;
    let (mut app, _rover) = setup(source);

    tick(&mut app);

    let cfg = app.world().resource::<SimConfig>();
    assert_eq!(cfg.speed, 9.5, "set_setting should write the f64 resource field");
    assert_eq!(cfg.steps, 7, "set_setting should write the i64 resource field");
    assert_eq!(
        app.world().resource::<CapturedData>().0,
        vec![7],
        "get_setting should read back the value just written (native round-trip)"
    );
}

/// A bad path/type returns `false` (logged, not a panic), so a scenario can
/// branch on the result, and the target stays untouched.
#[test]
fn set_verb_reports_failure_and_leaves_target_unchanged() {
    let source = r#"
        fn on_start(me) {
            if !set(me, "Knob.nope", 1.0) { emit("SET_FAILED"); }
            if !set_setting("Nonexistent.x", 1.0) { emit("SETTING_FAILED"); }
        }
    "#;
    let (mut app, rover) = setup(source);
    app.world_mut().entity_mut(rover).insert(Knob::default());

    tick(&mut app);

    let events = &app.world().resource::<EventLog>().0;
    assert!(events.iter().any(|e| e == "SET_FAILED"), "missing field → false; got {events:?}");
    assert!(events.iter().any(|e| e == "SETTING_FAILED"), "missing resource → false; got {events:?}");
    let knob = app.world().entity(rover).get::<Knob>().expect("Knob present");
    assert_eq!(knob.gain, 0.0, "a failed set must not mutate the component");
}

// ── Structural mutation: add / remove components, despawn entities ───────────

/// `add(id, "Comp", #{fields})` inserts a reflected component built from its
/// default + the field map — the C of CRUD — on an entity that lacked it.
#[test]
fn add_verb_inserts_reflected_component() {
    let source = r#"
        fn on_start(me) {
            add(me, "Knob", #{ gain: 5.0, armed: true, label: "live" });
        }
    "#;
    let (mut app, rover) = setup(source);
    assert!(app.world().entity(rover).get::<Knob>().is_none(), "rover starts without Knob");

    tick(&mut app);

    let knob = app.world().entity(rover).get::<Knob>().expect("add() should insert Knob");
    assert_eq!(knob.gain, 5.0);
    assert!(knob.armed);
    assert_eq!(knob.label, "live");
}

/// `remove(id, "Comp")` strips a component the entity had.
#[test]
fn remove_verb_strips_component() {
    let source = r#"fn on_start(me) { remove(me, "Knob"); }"#;
    let (mut app, rover) = setup(source);
    app.world_mut().entity_mut(rover).insert(Knob { gain: 1.0, ..default() });

    tick(&mut app);

    assert!(
        app.world().entity(rover).get::<Knob>().is_none(),
        "remove() should strip the Knob component"
    );
}

/// `despawn(id)` removes another entity entirely — gone from the world.
/// (Registry/replication cleanup is `sync_entity_registry` / `broadcast_despawns`'
/// job off the `GlobalEntityId` removal, exercised in the api/networking suites,
/// not wired into this minimal scripting harness.)
#[test]
fn despawn_verb_removes_entity() {
    const VICTIM_GID: u64 = 8888;
    let source = format!(r#"fn on_start(me) {{ despawn({VICTIM_GID}); }}"#);
    let (mut app, _rover) = setup(&source);
    let victim = spawn_typed_rover(&mut app, VICTIM_GID, 50.0);
    assert!(app.world().get_entity(victim).is_ok(), "victim exists before tick");

    tick(&mut app);

    assert!(
        app.world().get_entity(victim).is_err(),
        "despawn() should remove the entity from the world"
    );
}

// ── Deterministic RNG: rand() is a pure function of (entity, tick, call) ─────

/// `rand()` is reproducible — the SAME entity at the SAME tick draws the SAME
/// sequence across independent runs (networking-safe, replayable) — yet advances
/// within a tick and varies across ticks.
#[test]
fn rand_is_deterministic_across_runs_and_advances() {
    let source = r#"
        fn on_tick(me) {
            cmd("Report", #{ value: (rand() * 1000000.0).to_int() });
            cmd("Report", #{ value: (rand() * 1000000.0).to_int() });
        }
    "#;

    // One fresh app, pinned to `tick_val`, ticked once → the drawn ints.
    let run_once = |tick_val: u64| -> Vec<i64> {
        let (mut app, _rover) = setup(source);
        app.world_mut().insert_resource(lunco_core::SimTick(tick_val));
        tick(&mut app);
        app.world().resource::<CapturedData>().0.clone()
    };

    let a_t1 = run_once(1);
    let b_t1 = run_once(1); // independent run, same entity + tick
    let a_t2 = run_once(2); // same entity, different tick

    assert_eq!(a_t1.len(), 2, "two rand draws per tick");
    assert_ne!(a_t1[0], a_t1[1], "rand() must advance within a tick");
    assert_eq!(a_t1, b_t1, "same entity+tick → identical sequence across runs (deterministic)");
    assert_ne!(a_t1, a_t2, "a different tick must draw a different stream");
}
