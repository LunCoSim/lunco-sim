// The sandbox's admitted physical rover supplies a root, suspension and wheel.
// Observe the shared mass ports after a solver mass edit and a live island split.
const ROOT = "/SandboxScene/Skid_Physical_1";
const PART = ROOT + "/Suspension_FL";
const WHEEL = PART + "/Wheel_FL";
const JOINT = ROOT + "/Suspension_FL_Slider";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [],
       entity_reads: [find_path(ROOT), find_path(PART), find_path(WHEEL)],
       entity_writes: [find_path(PART), find_path(JOINT)],
       query_reads: ["ReadPorts"], required_inputs: [] }
}

fn mass_value(target, name) {
    let record = read_ports(target, [name]);
    if record == () || record.ports == () { return (); }
    for port in record.ports {
        if port.name == name { return port.value; }
    }
    ()
}

fn near_mass(actual, expected) {
    actual != () && expected != () && (actual - expected).abs() <= 1e-9
}

fn finish(failure) {
    if !this.done {
        this.done = true;
        if failure != "" { this.failures.push(failure); }
        report_verdict(this.failures, "JOINT ISLAND MASS", "JOINT_ISLAND_MASS");
    }
}

fn on_start(me, ctx) {
    this.root = find_path(ROOT);
    this.part = find_path(PART);
    this.wheel = find_path(WHEEL);
    this.joint = find_path(JOINT);
    this.done = false;
    this.failures = [];
    this.phase = 0;
    this.elapsed = 0.0;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 5.0 { this.finish("mass projection did not finish within five simulation seconds"); return; }
    if this.phase == 0 {
        this.total = mass_value(this.root, "dynamic_joint_island_mass_kg");
        this.part_mass = mass_value(this.part, "mass");
        this.wheel_mass = mass_value(this.wheel, "mass");
        if this.total == () || this.part_mass == () || this.wheel_mass == () {
            this.finish("the admitted physical assembly has no complete solver mass sample"); return;
        }
        this.failures.push(t_true(this.total > this.part_mass + this.wheel_mass,
            "the root sample does not include the articulated assembly"));
        this.failures.push(t_true(near_mass(mass_value(this.part, "dynamic_joint_island_mass_kg"), this.total),
            "connected members do not share one island mass"));
        let rejected = write_ports(this.part, [["mass", -1.0]]);
        this.failures.push(t_true(!rejected.ok, "negative solver mass was admitted"));
        this.failures.push(t_true(near_mass(mass_value(this.part, "mass"), this.part_mass),
            "rejected mass changed the solver sample"));
        let accepted = write_ports(this.part, [["mass", this.part_mass * 2.0]]);
        this.failures.push(t_true(accepted.ok, "valid solver mass update was rejected"));
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        this.failures.push(t_true(near_mass(mass_value(this.root, "dynamic_joint_island_mass_kg"),
            this.total + this.part_mass), "the root retained a stale mass after the solver edit"));
        let detached = cmd("DetachJoint", #{ target: this.joint, intent: "Interactive" });
        this.failures.push(t_true(detached.ok, "the suspension detach command was rejected"));
        this.phase = 2;
        this.split_at = this.elapsed;
        return;
    }
    // The retirement owner applies the queued detach at its Update boundary.
    if this.phase == 2 && this.elapsed >= this.split_at + 1.0 {
        let isolated = this.part_mass * 2.0 + this.wheel_mass;
        this.failures.push(t_true(near_mass(mass_value(this.part, "dynamic_joint_island_mass_kg"), isolated),
            "the detached suspension/wheel island retained the root mass"));
        this.failures.push(t_true(near_mass(mass_value(this.root, "dynamic_joint_island_mass_kg"),
            this.total + this.part_mass - isolated), "the root retained detached members"));
        this.failures.push(t_true(mass_value(this.root, "dynamic_joint_island_mass_valid") == 1.0
            && mass_value(this.part, "dynamic_joint_island_mass_valid") == 1.0,
            "a complete split island lost its valid-mass status"));
        this.finish("");
    }
}
