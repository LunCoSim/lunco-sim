// Verify a parked physical rover with both native and shared tire contacts.
const TEST_KIND = "headless";
const ROOT = "/AckermannParity";
const ROVER = ROOT + "/RoverPhysical";

fn simulation_dependencies(me, ctx) {
    let reads = [find_path(ROVER)];
    for side in ["FL", "FR", "RL", "RR"] {
        reads.push(find_path(ROVER + "/Suspension_" + side + "/Wheel_" + side));
    }
    #{modelica_entities: [find_path(ROVER)], entity_reads: reads,
      entity_writes: [find_path(ROVER)], query_reads: ["QueryPhysicsState"], required_inputs: []}
}

fn on_start(me, ctx) {
    this.start = (); this.initial_tick = (); this.maximum_drift = 0.0;
    this.contacts = [false, false, false, false]; this.done = false;
    let rover = find_path(ROVER);
    let result = brake(rover);
    if result == () || result.ok != true { throw "Parking command rejected"; }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    let rover = find_path(ROVER);
    if this.start == () {
        if sim_tick() < 180 { return; }
        this.start = world_pos(rover); this.initial_tick = sim_tick();
    }
    this.maximum_drift = max(this.maximum_drift, vlen(vsub(world_pos(rover), this.start)));
    let index = 0;
    for side in ["FL", "FR", "RL", "RR"] {
        let wheel = find_path(ROVER + "/Suspension_" + side + "/Wheel_" + side);
        let contact = contact_debug(wheel);
        if contact != () && contact.total_normal_impulse > 0.0 { this.contacts[index] = true; }
        index += 1;
    }
    if sim_tick() - this.initial_tick < 600 { return; }
    let native = find_path(ROVER + "/Suspension_FL/Wheel_FL");
    let shared_wheel = find_path(ROVER + "/Suspension_FR/Wheel_FR");
    let state = query("QueryPhysicsState", #{id: rover});
    this.done = true;
    print("MIXED_TIRE_PARKING_DRIFT_M " + this.maximum_drift);
    report_verdict([
        t_true(get(native, "JointedWheelTire.radius") == ()
            && get(shared_wheel, "JointedWheelTire.radius") != (),
            "one wheel uses native friction while another uses the shared tire"),
        t_true(this.contacts[0] && this.contacts[1] && this.contacts[2] && this.contacts[3],
            "all four wheels have carried normal load"),
        t_true(state.body_mode == "dynamic", "parked rover remains dynamic"),
        t_true(this.maximum_drift < 0.05, "mixed-support parking drift stays below five centimetres"),
    ], "MIXED TIRE PARKING", "MIXED_TIRE_PARKING");
}
