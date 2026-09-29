//! First Drive tutorial — orchestrator attached to /FirstDrive/Scenario.
//!
//! A declarative `mission(me)`: each objective advances when the student does
//! the real thing, not on a timer. Possession is checked against the rover's
//! live controller; arrival comes from the flag's generic Avian sensor event.
//! The engine auto-publishes the objectives checklist to the tutorial HUD.

fn on_start(me, ctx) {
    // The catalog-selected scene is mounted before this script starts. This script
    // only teaches and verifies the rover actions in that world.
    hint("Select the rover. Click Possess in Inspector or Command Deck.");
    notify_kind("Tutorial: First Drive", "info");

    this.rover = find_path("/FirstDrive/Rover");
    this.flag_sensor = find_path("/FirstDrive/Flag/Trigger");
    if this.rover < 0 || this.flag_sensor < 0 {
        notify_kind("First Drive cannot resolve its rover or waypoint sensor.", "error");
        return;
    }

    // Startup does not replay sensor events from before on_start. Read current
    // Avian occupancy once, then consume generic enter events for later arrival.
    let occupants = query("SensorOccupants", #{ sensor: this.flag_sensor });
    if occupants == () {
        notify_kind("First Drive cannot read waypoint sensor occupancy: no query result.", "error");
        return;
    }
    if occupants.ok == false {
        notify_kind("First Drive cannot read waypoint sensor occupancy: " + occupants.error, "error");
        return;
    }
    if occupants.entrants == () {
        notify_kind("First Drive waypoint occupancy has no entrant list.", "error");
        return;
    }
    for entrant in occupants.entrants {
        if first_drive_entity_within(entrant, this.rover) {
            emit("first_drive_waypoint_reached", entrant);
            break;
        }
    }
}

fn simulation_dependencies(me, ctx) {
    let rover = find_path("/FirstDrive/Rover");
    let flag = find_path("/FirstDrive/Flag");
    let sensor = find_path("/FirstDrive/Flag/Trigger");
    if rover < 0 || flag < 0 || sensor < 0 {
        throw "First Drive requires its authored rover, flag, and waypoint sensor before simulation starts";
    }
    #{ modelica_entities: [rover], entity_reads: [flag, sensor], entity_writes: [],
       query_reads: ["SensorOccupants"], required_inputs: [] }
}

fn first_drive_entity_within(entity, ancestor) {
    if entity == () || ancestor < 0 { return false; }
    let current = entity.to_int();
    if current < 0 { return false; }
    for depth in 0..32 {
        if current == ancestor { return true; }
        let next = parent(current);
        if next == () || next < 0 { return false; }
        current = next;
    }
    false
}

fn first_drive_possessed(me) {
    hint("Drive with " + input_hint("forward") + " / " + input_hint("backward") + ". Steer with " + input_hint("left") + " / " + input_hint("right") + ".\n\nStop at the glowing flag.");
}

fn first_drive_finished(me) {
    notify_kind("You reached the flag — nice driving!", "success");
}

fn first_drive_controlled(me) {
    // Headless acceptance runs have no local input controller. Their authored
    // task takes the rover through the same target and arrival objectives.
    if is_unattended() { return true; }
    let rover = find_path("/FirstDrive/Rover");
    rover >= 0 && controller(rover) != ()
}

fn mission(me, ctx) {
    // Named functions are used for the live possession check. Arrival is an
    // event objective so no callback needs to read scenario state indirectly.
    [
        // Step 1 — take control. A request is not success; live controller state
        // must show that this rover is actually owned.
        objective("possess", #{
            text: "Select the rover and click Possess in Inspector or Command Deck",
            done: Fn("first_drive_controlled"),
            on_complete: Fn("first_drive_possessed"),
        }),
        // Step 2 — the exact flag sensor must report entry by this rover (or
        // one of its colliders). The event is the arrival fact.
        objective("reach_flag", #{
            text: "Drive to the glowing flag and stop there",
            requires: ["possess"],
            requires_event: "first_drive_waypoint_reached",
            on_complete: Fn("first_drive_finished"),
        }),
    ]
}

fn on_event(me, evt, ctx) {
    if sensor_entered(evt, this.flag_sensor)
        && first_drive_entity_within(evt.value, this.rover) {
        emit("first_drive_waypoint_reached", evt.value);
    }

    // The engine emits MISSION_COMPLETE once every objective is done.
    if evt.name == "MISSION_COMPLETE" {
        hint("You took control and reached the flag.\n\nClick Continue to start Moon Driving Basics, or choose another lesson from Tutorials.");
        notify_kind("First Drive complete", "success");
    } else if evt.name == "cmd:ReleaseControlSource" {
        // Releasing the avatar only relinquishes authority. Stop this lesson's
        // rover endpoint explicitly so it cannot coast away from the waypoint.
        let rover = find_path("/FirstDrive/Rover");
        if rover >= 0 {
            let stopped = cmd("ReleaseControl", #{ target: rover });
            if stopped == () || stopped.ok != true {
                notify_kind("First Drive could not safely stop the rover after control was released.", "error");
            }
        }
    }
}

// UNATTENDED guidance runs alongside the mission for headless acceptance.
// Windowed sessions leave control to the student. Modelica owns continuous
// driving; entry into the Avian sensor, not a script distance threshold, ends
// the approach and tells this task when to brake.

fn first_drive_publish_waypoint(me) {
    let rover = find_path("/FirstDrive/Rover");
    let flag = find_path("/FirstDrive/Flag");
    let target = world_pos(flag);
    if rover < 0 || flag < 0 || target == () {
        notify_kind("First Drive could not resolve the rover or waypoint target.", "error");
        return;
    }
    // Modelica's radius shapes deceleration only. Arrival is the semantic
    // event produced from the generic Avian sensor transition.
    let result = modelica_waypoint(rover, target, 1.0, 1.0, 0.0);
    if result == () || result.ok != true {
        let detail = if result == () { "no command result" } else { result.error };
        notify_kind("First Drive could not publish rover guidance: " + detail, "error");
    }
}

fn first_drive_brake(me) {
    modelica_waypoint_stop(find_path("/FirstDrive/Rover"))
}

fn task(me, ctx) {
    if !is_unattended() { return; }   // a human is watching: they drive
    seq([
        wait(1.0),
        once(|m| first_drive_publish_waypoint(m)),
        wait_for("first_drive_waypoint_reached"),
        once(|m| first_drive_brake(m)),
        wait(2.0),
    ])
}
