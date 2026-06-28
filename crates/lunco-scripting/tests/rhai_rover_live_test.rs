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
    on_command, register_commands, ActiveCommandId, Command, GlobalEntityId, TelemetryEvent,
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

register_commands!(on_drive, on_brake);

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

/// Full setup: app + rover with `source` attached directly as a ScriptDocument +
/// ScriptedModel (bypasses the RunScenario command — used to test the runtime).
fn setup(source: &str) -> (App, Entity) {
    let mut app = build_app();
    let rover = spawn_rover(&mut app);

    let doc_id = DocumentId::new(1);
    let doc = ScriptDocument {
        id: 1,
        generation: 0,
        language: ScriptLanguage::Rhai,
        source: source.to_string(),
        inputs: vec![],
        outputs: vec![],
    };
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
