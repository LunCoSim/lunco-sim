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
    app.add_observer(lunco_luncosim_edit::commands::on_detach_joint);
    app.register_type::<lunco_luncosim_edit::commands::DetachJoint>();

    let joint_entity = app.world_mut().spawn_empty().id();
    assert!(app.world().get_entity(joint_entity).is_ok());

    app.world_mut()
        .trigger(lunco_luncosim_edit::commands::DetachJoint {
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
fn zone_enter_marks_the_waypoint_reached_without_deleting_it() {
    let mut app = App::new();

    // Initialize required resources and register event / types
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    // Provides `DocumentRegistry<UsdDocument>` — the arrival system resolves
    // the marker's document to author its runtime-layer flag.
    app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
    app.init_resource::<lunco_usd::twin_projection::DocBackedTwinScenes>();
    app.init_resource::<lunco_workspace::WorkspaceResource>();
    app.init_resource::<lunco_api::registry::ApiEntityRegistry>();
    app.register_type::<lunco_usd::commands::ApplyUsdOp>();
    // The arrival bus: avian hands CollisionStart to this system as a message.
    app.init_resource::<bevy::ecs::message::Messages<avian3d::prelude::CollisionStart>>();
    // The system under test: arrival driven by the marker's own trigger zone,
    // scheduled in FixedPostUpdate (after the physics writeback).
    app.add_systems(
        FixedPostUpdate,
        lunco_luncosim_edit::ui::checkpoint_click::mark_reached_waypoints_on_enter,
    );

    // Set active_document in the workspace
    use lunco_doc::DocumentId;
    let doc_id = DocumentId(1);
    app.world_mut()
        .resource_mut::<lunco_workspace::WorkspaceResource>()
        .0
        .active_document = Some(doc_id);

    // Spawn a vessel entity with BehaviorXml and UsdPrimPath
    // Waypoints are authored MARKER PRIMS; a mission leg targets one by path.
    const MARKER: &str = "/SandboxScene/Route/W0";
    let xml_content = format!(
        r#"<root BTCPP_format="4" main_tree_to_execute="MainTree">
  <BehaviorTree ID="MainTree">
    <Repeat num_cycles="-1">
      <Sequence>
        <Action ID="drive_to" target="{MARKER}"/>
        <Action ID="drive_to" target="/SandboxScene/Route/W1"/>
      </Sequence>
    </Repeat>
  </BehaviorTree>
</root>"#
    );

    let vessel_entity = app
        .world_mut()
        .spawn((
            lunco_autopilot::usd_tree::BehaviorXml(xml_content.clone()),
            lunco_usd_bevy::UsdPrimPath {
                stage_handle: Default::default(),
                path: "/SandboxScene/Skid_Raycast_2".to_string(),
            },
            Transform::from_xyz(10.0, 0.0, 20.0),
        ))
        .id();

    // The marker's trigger ZONE — arrival is this zone reporting an entrant, not
    // a distance the editor measured itself.
    let zone = app
        .world_mut()
        .spawn((
            lunco_core::TriggerZone("waypoint".to_string()),
            lunco_usd_bevy::UsdPrimPath {
                stage_handle: Default::default(),
                path: format!("{MARKER}/Zone"),
            },
            avian3d::prelude::Sensor,
        ))
        .id();

    // The vessel's chassis enters the zone: avian's narrow phase emits this
    // CollisionStart; the test hands the same message to the system under test.
    app.world_mut()
        .resource_mut::<bevy::ecs::message::Messages<avian3d::prelude::CollisionStart>>()
        .write(avian3d::prelude::CollisionStart {
            collider1: zone,
            collider2: vessel_entity,
            body1: None,
            body2: Some(vessel_entity),
        });

    // The system is scheduled in FixedPostUpdate; drive it there (the same
    // manual-schedule pattern as lunco-autopilot/tests/authority.rs).
    app.init_schedule(FixedUpdate);
    app.world_mut().run_schedule(FixedUpdate);
    app.world_mut().run_schedule(FixedPostUpdate);

    // "Reached" is LIVE-ONLY in the ECS: recorded in `ReachedWaypoints`, never
    // written into the XML. The mission keeps its leg — reaching a waypoint does
    // not delete it — and the marker USD is not rewritten.
    let updated_xml = app
        .world()
        .get::<lunco_autopilot::usd_tree::BehaviorXml>(vessel_entity)
        .unwrap();
    assert_eq!(
        updated_xml.0, xml_content,
        "the behaviour XML must be untouched — 'reached' is not authored into it"
    );

    let reached = app
        .world()
        .get::<lunco_autopilot::usd_tree::ReachedWaypoints>(vessel_entity)
        .expect("vessel must have gained a ReachedWaypoints component");
    assert!(
        reached.0.contains(MARKER),
        "the entered marker must be recorded, so the compiled tree advances"
    );
    assert!(
        !reached.0.contains("/SandboxScene/Route/W1"),
        "a waypoint whose zone was never entered must not be marked reached"
    );
}
