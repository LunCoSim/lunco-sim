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

fn runtime_waypoint_fail(detail) {
    report_verdict([detail], "RUNTIME WAYPOINT", "RUNTIME_WAYPOINT");
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
            position: [0.0, 1.0, 0.0],
        });
        if !result.ok {
            if this.elapsed <= 10.0 { return; }
            this.done = true;
            runtime_waypoint_fail("SpawnEntity failed: " + result.error);
            return;
        }
        this.spawn_issued = true;
        return;
    }

    if this.rover < 0 {
        // Runtime spawns receive unique authored prim paths and their human
        // labels are not the identity contract. Resolve authored catalog identity.
        this.rover = find_catalog("ackermann_rover");
        if this.rover < 0 {
            if this.elapsed > 10.0 {
                this.done = true;
                runtime_waypoint_fail("spawned Ackermann rover did not materialize");
            }
            return;
        }
    }

    if !this.waypoint_issued {
        let result = cmd("AddRuntimeWaypoint", #{
            target: this.rover,
            // Put the sensor ahead of the spawn point. Spawning it on top of
            // the rover would be initial penetration, not a CollisionStart
            // enter event, and would leave this acceptance test unproven.
            position: [0.0, 0.0, 8.0],
        });
        if !result.ok {
            this.done = true;
            runtime_waypoint_fail("AddRuntimeWaypoint failed: " + result.error);
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

    if this.elapsed > 120.0 {
        this.done = true;
        let detail = "RuntimeWaypointStatus query unavailable";
        if status != () {
            detail = "marker_count=" + status.marker_count +
                " reached_count=" + status.reached_count +
                " arrival_events=" + this.arrival_events;
        }
        runtime_waypoint_fail("runtime waypoint did not arrive: " + detail);
    }
}
