// @timing presentation
// Lifecycle commands remain usable without admitting a physics cycle.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: [], required_inputs: []}
}
fn on_start(me, ctx) {
    this.before = clock_snapshot();
    this.body = find_path("/SandboxScene/CubeB");
    this.position = world_pos3(this.body);
    let result = cmd("DetachJoint", #{target: find_path("/SandboxScene/Hinge"), intent: "Interactive"});
    this.accepted = result != () && result.ok == true;
}
fn detached() {
    let joint = find_path("/SandboxScene/Hinge");
    joint == () || joint < 0
}
fn finish(me) {
    let now = clock_snapshot();
    let position = world_pos3(this.body);
    report_verdict([
        t_true(this.accepted, "paused execution accepts the shared detach command"),
        t_true(detached(), "the lifecycle owner retires the paused joint"),
        t_true(now.sim_tick == this.before.sim_tick
            && now.physics_elapsed_s == this.before.physics_elapsed_s,
            "retirement admits no extra physics time"),
        t_true(position.x == this.position.x && position.y == this.position.y
            && position.z == this.position.z
            && query("QueryPhysicsState", #{id: this.body}).physics_state_ready == true,
            "the endpoint remains admitted at its frozen pose")
    ], "PAUSED JOINT DETACH", "PAUSED_JOINT_DETACH");
}
fn task(me, ctx) {
    seq([wait_until(|m| detached()), once(Fn("finish"))])
}
