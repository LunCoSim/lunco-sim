// Disabled rigid bodies retain ray queries without creating static-static solver edges.
fn simulation_dependencies(me, ctx) {
    let rover = find_path("/WheelAttachmentContract/Rover");
    let reads = vessel_descendants(rover);
    reads.push(rover);
    #{ modelica_entities: [],
       entity_reads: reads,
       entity_writes: [], query_reads: ["PhysicsPerformance", "GetReadiness"],
       required_inputs: [] }
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
    this.start = world_pos(find_path("/WheelAttachmentContract/Rover"));
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.2 { return; }
    let rover = find_path("/WheelAttachmentContract/Rover");
    let state = query("QueryPhysicsState", #{ id: rover });
    let performance = query("PhysicsPerformance", #{});
    let readiness = query("GetReadiness", #{});
    let contacts = 0;
    for contact in state.wheel_contacts {
        if contact.hit_available { contacts += 1; }
    }
    let position = world_pos(rover);
    let displacement = 0.0;
    for axis in 0..3 { displacement += abs(position[axis] - this.start[axis]); }
    this.done = true;
    report_verdict([
        t_true(state.body_mode == "static" && !state.rigid_body_disabled,
            "authored disabled body is admitted as static collision geometry"),
        t_true(displacement < 0.000001,
            "static wheel carrier retains its admitted pose"),
        t_true(contacts == 4,
            "all four static wheels still query their support"),
        t_true(performance.joint_graph_edges == 0,
            "static wheel contacts add no suspension stops to native islands"),
        t_true(performance.step_number > 0 && readiness.ready && !readiness.faulted,
            "native physics continues without an admission fault")
    ], "STATIC WHEEL CONTACT", "STATIC_WHEEL_CONTACT");
}
