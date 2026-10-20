// A contact on a light attached body must stop the complete hard-jointed mass.
const TEST_KIND = "headless";
const TOLERANCE_M = 0.000001;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [],
      entity_reads: [find_path("/World/A"), find_path("/World/B"),
                     find_path("/World/Free"), find_path("/World/Wall")],
      entity_writes: [], query_reads: ["QueryPhysicsState", "QueryUsdPrim"], required_inputs: []}
}

fn on_start(me, ctx) {
    this.start = sim_tick();
    this.done = false;
    let foot = find_path("/World/A");
    let root = find_path("/World/B");
    this.offset = world_pos3(foot) - world_pos3(root);
    let radius = query("QueryUsdPrim", #{path: "/World/A", attrs: ["radius"]}).attrs.radius;
    let bounds = query("QueryUsdPrim", #{path: "/World/Wall", attrs: [], collision_bounds: true}).collision_bounds;
    this.foot_limit = bounds.min[0] - radius;
    this.root_limit = this.foot_limit - this.offset.x;
    this.free_limit = bounds.max[0] + radius;
}

fn on_tick(me, ctx) {
    if this.done || sim_tick() - this.start < 3 { return; }
    this.done = true;
    let foot = find_path("/World/A");
    let root = find_path("/World/B");
    let free = find_path("/World/Free");
    let wall = find_path("/World/Wall");
    let a = world_pos3(foot);
    let b = world_pos3(root);
    let f = world_pos3(free);
    report_verdict([
        t_true(query("QueryPhysicsState", #{id: foot}).physics_state_ready,
               "the contacting body remains admitted"),
        t_true(query("QueryPhysicsState", #{id: root}).physics_state_ready,
               "the attached heavy body remains admitted"),
        t_true((a.x - this.foot_limit).abs() <= TOLERANCE_M,
               "the light body's continuous contact closes at the wall"),
        t_true((b.x - this.root_limit).abs() <= TOLERANCE_M,
               "the contact stops the attached heavy body"),
        t_true(vlen(a - b - this.offset) < TOLERANCE_M,
               "the hard attachment closes with the contact"),
        t_true(contact_debug(foot, wall).max_penetration < TOLERANCE_M,
               "the final native contact is within the shared position tolerance"),
        t_true(f.x > this.free_limit && f.x > b.x + this.offset.x,
               "an unconnected body does not inherit the contact response"),
    ], "Continuous articulated mass", "CONTINUOUS_ARTICULATED_MASS");
}
