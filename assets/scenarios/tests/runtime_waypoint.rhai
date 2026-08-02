// Production/headless acceptance test for a spawned rover's runtime waypoint.
// The command and arrival event are the same surfaces used by GUI Alt+LMB.

fn on_start(me) {
    this.elapsed = 0.0;
    this.spawn_issued = false;
    this.waypoint_issued = false;
    this.rover = -1;
    this.arrival_events = 0;
    this.done = false;
    print("[runtime-waypoint] -- spawned rover marker + collision arrival --");
}

fn on_event(me, evt) {
    if evt.name == "waypoint.reached" && evt.source == this.rover {
        this.arrival_events += 1;
        print("[runtime-waypoint] collision arrival event: " + evt.value);
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    // SpawnCatalog is populated asynchronously from USD.  Keep the public
    // command intact, but do not turn that normal startup race into a false
    // acceptance failure.
    if !this.spawn_issued && this.elapsed >= 1.0 {
        let result = cmd("SpawnEntity", #{
            entry_id: "ackermann_rover",
            position: [0.0, 0.0, 0.0],
        });
        if !result.ok {
            if this.elapsed <= 10.0 { return; }
            this.done = true;
            report_verdict(["SpawnEntity failed: " + result.error],
                           "RUNTIME WAYPOINT", "RUNTIME_WAYPOINT");
            return;
        }
        this.spawn_issued = true;
        return;
    }

    if this.rover < 0 {
        // Runtime spawns are mounted as top-level scene children. Their catalog
        // display name is the root Name; descendants retain USD prim paths.
        this.rover = find("Ackermann Rover");
        if this.rover < 0 {
            if this.elapsed > 10.0 {
                this.done = true;
                report_verdict(["spawned Ackermann rover did not materialize"],
                               "RUNTIME WAYPOINT", "RUNTIME_WAYPOINT");
            }
            return;
        }
    }

    if !this.waypoint_issued {
        let result = cmd("AddRuntimeWaypoint", #{
            target: this.rover,
            // Ground-level marker origin; its shared Zone is lifted to the
            // rover's chassis height and has a 4 m arrival radius.
            position: [0.0, 0.0, 0.0],
        });
        if !result.ok {
            this.done = true;
            report_verdict(["AddRuntimeWaypoint failed: " + result.error],
                           "RUNTIME WAYPOINT", "RUNTIME_WAYPOINT");
            return;
        }
        this.waypoint_issued = true;
        return;
    }

    let status = query("RuntimeWaypointStatus", #{ vessel: this.rover });
    if status != () && status.marker_count >= 1 && status.reached_count >= 1 {
        this.done = true;
        let checks = [];
        checks.push(t_true(status.marker_count == 1,
            "expected one runtime marker, got " + status.marker_count));
        checks.push(t_true(status.reached_count == 1,
            "expected one collision-backed arrival, got " + status.reached_count));
        checks.push(t_true(this.arrival_events >= 1,
            "ReachedWaypoints changed without the waypoint.reached collision event"));
        report_verdict(checks, "RUNTIME WAYPOINT", "RUNTIME_WAYPOINT");
        return;
    }

    if this.elapsed > 20.0 {
        this.done = true;
        let detail = "RuntimeWaypointStatus query unavailable";
        if status != () {
            detail = "marker_count=" + status.marker_count +
                " reached_count=" + status.reached_count +
                " arrival_events=" + this.arrival_events;
        }
        report_verdict(["runtime waypoint did not arrive: " + detail],
                       "RUNTIME WAYPOINT", "RUNTIME_WAYPOINT");
    }
}
