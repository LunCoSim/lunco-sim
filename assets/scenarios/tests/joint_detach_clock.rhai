// A live detach must retire before the next physics tick, even when several
// complete cycles share one host frame. The other bodies must keep stepping.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [],
      entity_reads: [find_path("/SandboxScene/CubeA"), find_path("/SandboxScene/CubeB"), find_path("/SandboxScene/FreeCube")],
      entity_writes: [find_path("/SandboxScene/Hinge")], query_reads: [], required_inputs: []}
}
fn on_start(me, ctx) {
    this.start = sim_tick();
    this.joint = find_path("/SandboxScene/Hinge");
    this.body = find_path("/SandboxScene/CubeB");
    this.free = find_path("/SandboxScene/FreeCube");
    this.free_start = world_pos3(this.free);
    this.accepted = false;
    this.retired_next_tick = false;
    this.done = false;
    let rejected = cmd("DetachJoint", #{target: -1, intent: "Interactive"});
    this.invalid_rejected = rejected != () && rejected.ok == false;
}
fn on_tick(me, ctx) {
    if this.done { return; }
    let tick = sim_tick() - this.start;
    if tick == 1 {
        let result = cmd("DetachJoint", #{target: this.joint, intent: "Interactive"});
        this.accepted = result != () && result.ok == true;
    }
    if tick == 2 {
        let joint = find_path("/SandboxScene/Hinge");
        this.retired_next_tick = joint == () || joint < 0;
    }
    if tick < 3 { return; }
    this.done = true;
    report_verdict([
        t_true(this.invalid_rejected, "invalid detach rejects without changing the live joint"),
        t_true(this.accepted, "an admitted joint accepts the shared detach command"),
        t_true(this.retired_next_tick, "the next physics tick observes solver-safe retirement"),
        t_true(world_pos3(this.free).y < this.free_start.y
            && query("QueryPhysicsState", #{id: this.body}).physics_state_ready == true,
            "unrelated and released bodies continue native integration")
    ], "JOINT DETACH CLOCK", "JOINT_DETACH_CLOCK");
}
