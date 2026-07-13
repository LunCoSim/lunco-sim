//! Missions authored as BT.CPP XML + USD waypoint prims.
//!
//! The load-bearing property: a spatial leaf REFERENCES a waypoint prim by path, and
//! the compiler bakes that prim's live world position into the tree — so **dragging
//! the pin re-routes the rover**. That is what makes a waypoint an ordinary prim
//! (selectable, gizmo-draggable, journaled, persisted) instead of a bespoke
//! checkpoint domain.

use bevy::prelude::*;
use lunco_autopilot::usd_tree::{compile_behavior_xml, BehaviorXml, TargetBindings};
use lunco_autopilot::{AutopilotBehaviorSpec, BehaviorSpec};

/// A one-waypoint patrol whose `drive_to` names a prim instead of coordinates.
const XML: &str = r#"
<root BTCPP_format="4" main_tree_to_execute="MainTree">
  <BehaviorTree ID="MainTree">
    <Sequence>
      <Action ID="drive_to" target="/World/Behaviors/RoverPatrol/wp0" speed="0.6" radius="3"/>
      <Action ID="run_tool" tool="science::take_photo" args=""/>
    </Sequence>
  </BehaviorTree>
</root>
"#;

fn app() -> App {
    let mut app = App::new();
    app.add_systems(Update, compile_behavior_xml);
    app
}

/// Spawn a vessel carrying the tree, and a waypoint pin at `pin_pos` bound to the
/// path the tree names. Returns (vessel, pin).
fn scene(app: &mut App, pin_pos: Vec3) -> (Entity, Entity) {
    let pin = app
        .world_mut()
        .spawn((Transform::from_translation(pin_pos), GlobalTransform::from_translation(pin_pos)))
        .id();
    let mut bindings = TargetBindings::default();
    bindings
        .0
        .insert("/World/Behaviors/RoverPatrol/wp0".to_string(), pin);
    let vessel = app
        .world_mut()
        .spawn((BehaviorXml(XML.to_string()), bindings))
        .id();
    (vessel, pin)
}

/// The compiled `drive_to` target of the vessel's derived spec.
fn drive_target(app: &App, vessel: Entity) -> [f32; 3] {
    let spec = app
        .world()
        .get::<AutopilotBehaviorSpec>(vessel)
        .expect("vessel must have a derived spec");
    let BehaviorSpec::Sequence { children } = &spec.0 else {
        panic!("expected a sequence, got {:?}", spec.0);
    };
    match &children[0] {
        BehaviorSpec::DriveTo { target, .. } => *target,
        other => panic!("expected drive_to first, got {other:?}"),
    }
}

#[test]
fn ctrl_click_on_a_vessel_with_no_mission_creates_the_patrol_shell() {
    use lunco_autopilot::usd_tree::{append_waypoint_leaf, target_paths};
    let xml = append_waypoint_leaf(None, "/World/Behaviors/Rover_wp1").unwrap();
    assert_eq!(
        target_paths(&xml),
        vec!["/World/Behaviors/Rover_wp1".to_string()],
        "the first checkpoint must create a forever(sequence[drive_to]) mission that \
         REFERENCES the pin prim (not bake its coordinates)"
    );
}

#[test]
fn a_second_ctrl_click_appends_to_the_existing_route() {
    use lunco_autopilot::usd_tree::{append_waypoint_leaf, target_paths};
    let one = append_waypoint_leaf(None, "/W/B/wp1").unwrap();
    let two = append_waypoint_leaf(Some(&one), "/W/B/wp2").unwrap();
    assert_eq!(
        target_paths(&two),
        vec!["/W/B/wp1".to_string(), "/W/B/wp2".to_string()],
        "waypoints append IN ORDER"
    );
}

#[test]
fn the_editor_refuses_to_restructure_a_hand_authored_tree() {
    // A mission that isn't the plain forever(sequence[…]) patrol shape — say one a
    // human wrote in Groot2 — must be left alone rather than silently rewritten by a
    // stray Ctrl+click.
    use lunco_autopilot::usd_tree::append_waypoint_leaf;
    let handmade = r#"
    <root BTCPP_format="4" main_tree_to_execute="MainTree">
      <BehaviorTree ID="MainTree">
        <Fallback>
          <Action ID="brake"/>
        </Fallback>
      </BehaviorTree>
    </root>"#;
    assert!(
        append_waypoint_leaf(Some(handmade), "/W/B/wp1").is_err(),
        "a non-patrol tree must not be restructured by the editor"
    );
}

#[test]
fn waypoint_prim_position_is_baked_into_the_tree() {
    let mut app = app();
    let (vessel, _pin) = scene(&mut app, Vec3::new(10.0, 0.0, 3.0));
    app.update();
    assert_eq!(
        drive_target(&app, vessel),
        [10.0, 0.0, 3.0],
        "the drive_to target must come from the waypoint PRIM, not the XML"
    );
    // The tool leaf survived the bake untouched.
    let spec = app.world().get::<AutopilotBehaviorSpec>(vessel).unwrap();
    let BehaviorSpec::Sequence { children } = &spec.0 else { unreachable!() };
    assert!(
        matches!(&children[1], BehaviorSpec::RunTool { tool, .. } if tool == "science::take_photo"),
        "run_tool leaf must round-trip through the XML"
    );
}

#[test]
fn dragging_the_pin_reroutes_the_rover() {
    // THE point of putting waypoints in USD: moving the prim (which is what the
    // transform gizmo does) re-derives the mission. No checkpoint command, no bespoke
    // domain — just a prim that moved.
    let mut app = app();
    let (vessel, pin) = scene(&mut app, Vec3::new(10.0, 0.0, 3.0));
    app.update();
    assert_eq!(drive_target(&app, vessel), [10.0, 0.0, 3.0]);

    // Drag the pin.
    let moved = Vec3::new(-4.0, 0.0, 25.0);
    app.world_mut().entity_mut(pin).insert((
        Transform::from_translation(moved),
        GlobalTransform::from_translation(moved),
    ));
    app.update();

    assert_eq!(
        drive_target(&app, vessel),
        [-4.0, 0.0, 25.0],
        "moving the waypoint prim must recompile the route"
    );
}

#[test]
fn a_dangling_waypoint_reference_does_not_compile_a_route_to_the_origin() {
    // A tree naming a deleted (or not-yet-spawned) waypoint must keep its last good
    // route, NOT silently bake [0,0,0] and drive the rover into the world origin.
    let mut app = app();
    let (vessel, pin) = scene(&mut app, Vec3::new(10.0, 0.0, 3.0));
    app.update();
    assert_eq!(drive_target(&app, vessel), [10.0, 0.0, 3.0]);

    // Delete the pin and drop the binding, as despawning the prim would.
    app.world_mut().entity_mut(pin).despawn();
    app.world_mut()
        .entity_mut(vessel)
        .insert(TargetBindings::default());
    app.update();

    assert_eq!(
        drive_target(&app, vessel),
        [10.0, 0.0, 3.0],
        "an unresolved target must not compile — the previous route stands"
    );
}
