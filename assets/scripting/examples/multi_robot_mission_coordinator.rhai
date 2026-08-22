// Event-driven multi-robot coordinator.
//
// Attach exactly one copy to a mission-control entity, then attach
// multi_robot_mission_worker.rhai to each robot. The coordinator is the sole
// assignment authority: workers announce readiness, receive an identity-scoped
// assignment event, and report completion. No on_tick loop or shared mutable
// Rhai state is required.
//
// This is deliberately a coordinator protocol, not an atomic distributed claim
// service. Running two coordinators for the same mission is invalid; an atomic
// multi-agent claim primitive remains a host/Rust feature, because separate Rhai
// VMs cannot safely share a map.

const SITES = [
    [12.0, 0.0,  0.0],
    [12.0, 0.0, 25.0],
    [-10.0, 0.0, 25.0],
    [  0.0, 0.0,  0.0],
];

fn robot_key(robot) { robot.to_string() }

fn robot_is_assigned(assigned, robot) {
    let key = robot_key(robot);
    key in assigned && assigned[key] >= 0
}

fn assign_next(assigned, next_site, robot, site_count) {
    let key = robot_key(robot);
    if robot_is_assigned(assigned, robot) {
        return #{ assigned: assigned, next_site: next_site, assigned_site: assigned[key] };
    }
    if next_site >= site_count {
        return #{ assigned: assigned, next_site: next_site, assigned_site: () };
    }
    assigned[key] = next_site;
    #{ assigned: assigned, next_site: next_site + 1, assigned_site: next_site }
}

fn on_start(me) {
    subscribe("ROBOT_READY");
    subscribe("ROBOT_SITE_COMPLETE");
    this.next_site = 0;
    this.assigned = #{};
    this.completed = 0;
    print("coordinator: waiting for " + SITES.len() + " site assignments");
}

fn on_event(me, evt) {
    if evt.name == "ROBOT_READY" {
        let result = assign_next(this.assigned, this.next_site, evt.source, SITES.len());
        this.assigned = result.assigned;
        this.next_site = result.next_site;
        if result.assigned_site != () {
            emit("MISSION_ASSIGN:" + robot_key(evt.source) + ":" + result.assigned_site, true);
            print("coordinator: assigned robot " + evt.source + " site " + result.assigned_site);
        }
        return;
    }

    if evt.name == "ROBOT_SITE_COMPLETE" && robot_is_assigned(this.assigned, evt.source) {
        this.assigned[robot_key(evt.source)] = -1;
        this.completed += 1;
        emit("SITE_COMPLETE", this.completed);
        if this.completed >= SITES.len() {
            mission_checkpoint(me, "phase", "complete");
            emit("MISSION_COMPLETE", true);
            print("coordinator: mission complete");
        }
    }
}

fn mission(me) {
    [objective("all_sites", #{
        text: "All assigned sites complete",
        requires_event: "MISSION_COMPLETE",
    })]
}
