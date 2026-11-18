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
      entity_writes: reads, query_reads: ["QueryPhysicsState"], required_inputs: []}
}

fn on_start(me, ctx) {
    this.start = (); this.initial_tick = (); this.maximum_drift = 0.0;
    this.contacts = [false, false, false, false]; this.done = false;
    this.unpowered_tick = ();
    this.release_tick = ();
    let rover = find_path(ROVER);
    let result = brake(rover);
    if result == () || result.ok != true { throw "Parking command rejected"; }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    let rover = find_path(ROVER);
    if this.release_tick != () {
        let state = query("QueryPhysicsState", #{id: rover});
        if state.sleeping && sim_tick() - this.release_tick < 10 { return; }
        this.done = true;
        this.checks.push(t_true(!state.sleeping, "releasing the brake wakes the parked island"));
        report_verdict(this.checks, "MIXED TIRE PARKING", "MIXED_TIRE_PARKING");
        return;
    }
    if this.unpowered_tick != () {
        if sim_tick() - this.unpowered_tick < 120 { return; }
        let parked = query("QueryPhysicsState", #{id: rover});
        this.checks.push(t_true(parked.sleeping, "held brake with zero shaft torque permits island sleep"));
        let release = drive(rover, 0.0, 0.0);
        if release == () || release.ok != true { throw "Brake release rejected"; }
        this.release_tick = sim_tick();
        return;
    }
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
    print("MIXED_TIRE_PARKING_DRIFT_M " + this.maximum_drift);
    this.checks = [
        t_true(get(native, "JointedWheelTire.radius") == ()
            && get(shared_wheel, "JointedWheelTire.radius") != (),
            "one wheel uses native friction while another uses the shared tire"),
        t_true(this.contacts[0] && this.contacts[1] && this.contacts[2] && this.contacts[3],
            "all four wheels have carried normal load"),
        t_true(state.body_mode == "dynamic", "parked rover remains dynamic"),
        t_true(this.maximum_drift < 0.05, "mixed-support parking drift stays below five centimetres"),
    ];
    let neutral = [];
    for side in ["FL", "FR", "RL", "RR"] {
        let wheel = find_path(ROVER + "/Suspension_" + side + "/Wheel_" + side);
        neutral.push(#{target: wheel, writes: [["drive", 0.0]]});
    }
    let result = write_ports_batch(neutral);
    if result == () || result.ok != true { throw "Zero drive effort rejected: " + result; }
    this.unpowered_tick = sim_tick();
}
