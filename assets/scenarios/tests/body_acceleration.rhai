// Production gate: supported body -> applied force/torque -> free fall.
// The fixture uses the 50 kg body and lunar gravity in body_acceleration.usda.
const TEST_KIND = "headless";
const BODY = "/SandboxScene/Box50";
const G = 1.62;

fn simulation_dependencies(me, ctx) {
    let body = find_path(BODY);
    #{modelica_entities: [], entity_reads: [body], entity_writes: [body],
      query_reads: [], required_inputs: []}
}

fn optional_read(body, port) {
    try { return get(body, port); } catch (error) { return (); }
}

fn on_start(me, ctx) {
    this.body = find_path(BODY);
    this.mass = get(this.body,"mass");
    this.t = 0.0;
    this.done = false;
    this.lifting = false;
    this.released = false;
    this.support = 0;
    this.thrust = 0;
    this.freefall = 0;
    this.max_roll = 0.0;
    this.failures = [];
    this.failures.push(t_true(get(this.body, "acceleration_valid") < 0.5,
        "a body without a completed interval must be explicitly invalid"));
    this.failures.push(t_true(optional_read(this.body, "specific_force_magnitude") == (),
        "an unavailable accelerometer measurement must not be a fabricated zero"));
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.t += dt();
    let b = this.body;
    // Native force ports are a per-cycle additive sink, not a held command.
    let thrusting = this.t >= 1.2 && this.t < 2.0;
    cmd("SetPorts", #{target:b, writes:[
        ["force_y",if thrusting {this.mass*5.0} else {0.0}],
        ["torque_z",if thrusting {5.0} else {0.0}]]});
    if !this.lifting && this.t >= 1.2 {
        this.lifting = true;
    }
    if !this.released && this.t >= 2.0 {
        this.released = true;
    }
    if get(b,"specific_force_valid") > 0.5 {
        let net = get(b,"acceleration_magnitude");
        let proper = get(b,"specific_force_magnitude");
        let x = get(b,"specific_force_body_x");
        let y = get(b,"specific_force_body_y");
        let z = get(b,"specific_force_body_z");
        let magnitude = sqrt(x*x+y*y+z*z);
        this.failures.push(t_true(abs(magnitude-proper)<1.0e-9,
            "accelerometer magnitude must be invariant under body rotation"));
        this.max_roll = max(this.max_roll,abs(get(b,"roll")));
        if this.t > 0.3 && this.t < 0.9 {
            this.support += 1;
            this.failures.push(t_true(net<0.05 && abs(proper-G)<0.05,
                "supported body: near-zero net acceleration and one local g of support"));
        }
        if this.t > 1.5 && this.t < 1.9 {
            this.thrust += 1;
            this.failures.push(t_true(abs(net-(5.0-G))<0.05 && abs(proper-5.0)<0.05,
                "airborne applied force: net and gravity-removed acceleration must differ by gravity"));
        }
        if this.t > 2.3 && this.t < 2.8 {
            this.freefall += 1;
            this.failures.push(t_true(abs(net-G)<0.05 && proper<0.05,
                "free fall: nonzero net acceleration and near-zero accelerometer load"));
        }
    }
    if this.t >= 2.9 {
        this.done = true;
        this.failures.push(t_true(this.support>=12 && this.thrust>=12 && this.freefall>=12,
            "all three physical conditions must supply completed measurements"));
        this.failures.push(t_true(this.max_roll>0.01,
            "body attitude must change during the magnitude-invariance check"));
        report_verdict(this.failures,"BODY ACCELERATION","BODY_ACCELERATION");
    }
}
