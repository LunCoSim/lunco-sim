// Example coordinator scenario — demonstrates the EVENT channel and inter-script
// interaction. Attach to any entity (a disembodied "mission control" entity is
// fine; it never moves).
//
// Scripts never call each other directly — they are isolated VMs that interact
// ONLY through the world: here, the shared TelemetryEvent bus. patrol.rhai
// emits "WAYPOINT_REACHED"; this script reacts via on_event and, when the
// mission target is met, emits "MISSION_COMPLETE" for the next consumer (UI,
// API subscriber, ROS2 action result, or yet another script).
//
// Delivery is frame-delayed (event emitted on tick N arrives on tick N+1), so
// ordering is deterministic regardless of entity iteration order.

const TARGET = 6;   // checkpoints to clear before the mission is done

fn on_start(me) {
    this.count = 0;
    this.done = false;
    print("mission: armed, need " + TARGET + " checkpoints");
}

fn on_event(me, evt) {
    if this.done { return; }

    if evt.name == "WAYPOINT_REACHED" {
        this.count += 1;
        print("mission: checkpoint " + this.count + "/" + TARGET);

        if this.count >= TARGET {
            this.done = true;
            emit("MISSION_COMPLETE", true);
            print("mission: COMPLETE");
        }
    }
}
