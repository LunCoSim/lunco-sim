//! Documents the load-bearing fact behind the click-routing fix: two *global*
//! observers watching the same event BOTH run for a single trigger. That's why
//! selection (`on_scene_click_select`) and possession (`avatar_raycast_possession`)
//! must partition by keyboard modifier — Shift+click selects, plain click
//! possesses — rather than relying on one swallowing the click from the other.

use bevy::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static A_RAN: AtomicUsize = AtomicUsize::new(0);
static B_RAN: AtomicUsize = AtomicUsize::new(0);

#[derive(EntityEvent)]
struct MyClick {
    entity: Entity,
}

fn observer_a(_on: On<MyClick>) {
    A_RAN.fetch_add(1, Ordering::SeqCst);
}

fn observer_b(_on: On<MyClick>) {
    B_RAN.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn both_global_observers_run_for_one_event() {
    let mut app = App::new();
    app.add_observer(observer_a);
    app.add_observer(observer_b);
    let entity = app.world_mut().spawn_empty().id();
    app.world_mut().trigger(MyClick { entity });

    assert_eq!(A_RAN.load(Ordering::SeqCst), 1, "observer A must run");
    assert_eq!(
        B_RAN.load(Ordering::SeqCst),
        1,
        "observer B must run for the SAME event"
    );
}

#[test]
fn test_detach_joint_command() {
    let mut app = App::new();
    app.add_plugins(lunco_core::LunCoCorePlugin);
    app.add_observer(lunco_sandbox_edit::commands::on_detach_joint);
    app.register_type::<lunco_sandbox_edit::commands::DetachJoint>();

    let joint_entity = app.world_mut().spawn_empty().id();
    assert!(app.world().get_entity(joint_entity).is_ok());

    app.world_mut()
        .trigger(lunco_sandbox_edit::commands::DetachJoint {
            target: joint_entity,
            intent: lunco_core::EditIntent::Interactive,
        });

    // Flush commands to execute the observer
    app.world_mut().flush();

    assert!(
        app.world().get_entity(joint_entity).is_err(),
        "Joint entity must be despawned by DetachJoint command"
    );
}

#[test]
fn test_delete_reached_coordinate_waypoint() {
    let mut app = App::new();

    // Initialize required resources and register event / types
    app.init_resource::<lunco_workspace::WorkspaceResource>();
    app.register_type::<lunco_usd::commands::ApplyUsdOp>();

    // Setup a resource to store triggered ApplyUsdOp events
    #[derive(Default, Resource)]
    struct TriggeredOps(Vec<lunco_usd::commands::ApplyUsdOp>);
    app.insert_resource(TriggeredOps::default());

    app.add_observer(
        |trigger: On<lunco_usd::commands::ApplyUsdOp>, mut ops: ResMut<TriggeredOps>| {
            ops.0.push(trigger.event().clone());
        },
    );

    // Set active_document in the workspace
    use lunco_doc::DocumentId;
    let doc_id = DocumentId(1);
    app.world_mut()
        .resource_mut::<lunco_workspace::WorkspaceResource>()
        .0
        .active_document = Some(doc_id);

    // Spawn a vessel entity with BehaviorXml and UsdPrimPath
    let xml_content = r#"<root BTCPP_format="4" main_tree_to_execute="MainTree">
  <BehaviorTree ID="MainTree">
    <Repeat num_cycles="-1">
      <Sequence>
        <Action ID="drive_to" target="10.0;0.0;20.0"/>
        <Action ID="drive_to" target="30.0;0.0;40.0"/>
      </Sequence>
    </Repeat>
  </BehaviorTree>
</root>"#;

    let vessel_entity = app
        .world_mut()
        .spawn((
            lunco_autopilot::usd_tree::BehaviorXml(xml_content.to_string()),
            lunco_usd_bevy::UsdPrimPath {
                stage_handle: Default::default(),
                path: "/SandboxScene/Skid_Raycast_2".to_string(),
            },
            Transform::from_xyz(10.0, 0.0, 20.0), // Vessel is exactly at the first waypoint!
        ))
        .id();

    // Run the system
    let mut schedule = Schedule::new(Update);
    schedule.add_systems(lunco_sandbox_edit::ui::checkpoint_click::delete_reached_waypoints);
    schedule.run(app.world_mut());

    // "Reached" is LIVE-ONLY: it is recorded in the runtime `ReachedWaypoints`
    // component and must NEVER touch the XML or the document. The XML is left byte-for
    // byte alone, and NO ApplyUsdOp is emitted — otherwise the flag would be journaled,
    // saved into the .usda and survive a reload (which is what emptied the route and
    // made the autopilot drive forward instead of following it).
    let updated_xml = app
        .world()
        .get::<lunco_autopilot::usd_tree::BehaviorXml>(vessel_entity)
        .unwrap();
    assert_eq!(
        updated_xml.0, xml_content,
        "the behaviour XML must be untouched — 'reached' is not authored"
    );
    assert!(
        !updated_xml.0.contains("passed"),
        "no passed flag may ever be written into the XML"
    );

    // The reached waypoint is recorded in the runtime component; the next one is not.
    let reached = app
        .world()
        .get::<lunco_autopilot::usd_tree::ReachedWaypoints>(vessel_entity)
        .expect("vessel must have gained a ReachedWaypoints component");
    assert!(
        reached.0.contains("10.0;0.0;20.0"),
        "the reached waypoint must be recorded live-only"
    );
    assert!(
        !reached.0.contains("30.0;0.0;40.0"),
        "the not-yet-reached waypoint must not be recorded"
    );

    // Nothing may be authored: no journal / save / replication for a transient flag.
    let triggered = app.world().resource::<TriggeredOps>();
    assert!(
        triggered.0.is_empty(),
        "reaching a coordinate waypoint must author NOTHING to USD"
    );
}
