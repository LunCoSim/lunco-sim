// @timing presentation
// The driver supplies official Reference FMUs; native binaries stay outside
// authored assets. Presentation can observe a held simulation without ticks.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetReadiness", "ReadPorts"], required_inputs: []}
}
fn require(result) { if result.ok != true { throw(result.to_string()); } }
fn settled() {
    let clock = clock_snapshot();
    !clock.barrier_held && !clock.simulation_progress.held && !clock.simulation_cycle_active
        && clock.queued_simulation_steps.to_int() == 0
        && find("FMI Clock Decay") >= 0 && find("FMI Clock Bounce") >= 0
}
fn on_start(me, ctx) {
    this.checks = []; this.done = false;
    require(cmd("StepSimulation", #{hold: true}));
    require(cmd("LoadFmu", #{source: ctx.decay, name: "FMI Clock Decay", communication_ticks: 6}));
    require(cmd("LoadFmu", #{source: ctx.bounce, name: "FMI Clock Bounce", communication_ticks: 3}));
}
fn sample(me) {
    this.clock = clock_snapshot();
    this.decay_time = get(this.decay, "FmuParticipant.time_s");
    this.bounce_time = get(this.bounce, "FmuParticipant.time_s");
}
fn admitted(me) {
    this.decay = find("FMI Clock Decay"); this.bounce = find("FMI Clock Bounce");
    this.checks += [
        t_true(get(this.decay, "x") == 1.0, "FMU initialization publishes the real continuous state"),
        t_true(get(this.bounce, "h") == 1.0 && get(this.bounce, "v") == 0.0,
            "Event Mode initialization publishes the real ball state"),
        t_true(get(this.decay, "FmuParticipant.source_revision") != 0,
            "admission retains the exact archive revision"),
        t_true(get(this.decay, "FmuParticipant.fmu_can_restore_state"),
            "the external FMU declares its state restoration capability")
    ];
    let height = query("ReadPorts", #{api_id: this.bounce, port_names: ["h"]}).ports[0];
    let velocity = query("ReadPorts", #{api_id: this.bounce, port_names: ["v"]}).ports[0];
    this.checks += [
        t_true(height.metadata.unit.id == "m" && height.metadata.unit.definition.dimension_si == [1, 0, 0, 0, 0, 0, 0],
            "the shared port registry publishes the inherited FMI length unit"),
        t_true(velocity.metadata.unit.id == "m/s" && velocity.metadata.unit.definition.dimension_si == [1, 0, -1, 0, 0, 0, 0],
            "the shared port registry publishes the inherited FMI velocity unit"),
        t_true(height.metadata.unit.definition.scale_to_si == 1.0
            && height.metadata.unit.definition.angle_power == 0 && !height.metadata.unit.definition.relative_quantity,
            "the generic port wire retains the complete FMI conversion definition")
    ];
    let rad = engineering_unit("rad", [0, 0, 0, 0, 0, 0, 0], 1.0, 0.0).with_angle_power(1);
    let deg = engineering_unit("deg", [0, 0, 0, 0, 0, 0, 0], 0.017453292519943295, 0.0).with_angle_power(1);
    let scalar = engineering_unit("1", [0, 0, 0, 0, 0, 0, 0], 1.0, 0.0);
    let celsius = engineering_unit("degC", [0, 0, 0, 0, 1, 0, 0], 1.0, 273.15);
    let kelvin = engineering_unit("K", [0, 0, 0, 0, 1, 0, 0], 1.0, 0.0);
    this.checks += [
        t_true(abs(engineering_convert(180.0, deg, rad) - 3.141592653589793) < 1.0e-14,
            "the typed engineering surface converts angular quantities"),
        t_true(engineering_convert_or_none(1.0, rad, scalar) == (),
            "angular quantities cannot connect as dimensionless scalars"),
        t_true(engineering_convert(5.0, celsius.with_relative_quantity(true), kelvin.with_relative_quantity(true)) == 5.0,
            "relative temperature conversion ignores the absolute offset"),
        t_true(engineering_convert_or_none(5.0, celsius.with_relative_quantity(true), kelvin) == (),
            "relative and absolute quantities remain distinct at the shared boundary")
    ];
    this.sample(me);
    require(cmd("StepSimulation", #{steps: 30}));
}
fn verify_delta(me, ticks, decay_ticks, bounce_ticks) {
    let clock = clock_snapshot(); let quantum = this.clock.simulation_dt_s;
    this.checks += [
        t_true(clock.sim_tick - this.clock.sim_tick == ticks, "the master admits the exact complete tick count"),
        t_true(abs(get(this.decay, "FmuParticipant.time_s") - this.decay_time - decay_ticks * quantum) < 1.0e-12,
            "the slower external FMU follows the integer simulation clock"),
        t_true(abs(get(this.bounce, "FmuParticipant.time_s") - this.bounce_time - bounce_ticks * quantum) < 1.0e-12,
            "the event-driven external FMU follows the same simulation clock"),
        t_true(abs(clock.physics_elapsed_s - this.clock.physics_elapsed_s - ticks * this.clock.fixed_dt_s) < 1.0e-12,
            "native physics consumes those same master ticks")
    ];
}
fn bounced(me) {
    this.verify_delta(me, 30, 30, 30);
    this.checks += [
        t_true(get(this.decay, "x") > 0.55 && get(this.decay, "x") < 0.65,
            "the external continuous solver advances its decay equation"),
        t_true(get(this.bounce, "h") > 0.0 && get(this.bounce, "v") > 0.0,
            "the external ball handles its impact event and rebounds")
    ];
    require(cmd("SetTimeTransport", #{playing: false, rate: 4.0}));
    this.sample(me);
}
fn frozen(me) {
    this.verify_delta(me, 0, 0, 0);
    this.checks.push(t_true(clock_snapshot().transport_rate == 4.0,
        "speed controls alter demand without changing the integration quantum"));
    this.checks.push(t_true(cmd("SetFmuPaused", #{target: this.decay, paused: true}).ok,
        "the external participant accepts a local pause"));
    require(cmd("StepSimulation", #{steps: 6}));
}
fn locally_paused(me) {
    this.verify_delta(me, 6, 0, 6);
    this.checks.push(t_true(cmd("SetFmuPaused", #{target: this.decay, paused: false}).ok,
        "the external participant accepts a local resume"));
    this.sample(me);
    require(cmd("StepSimulation", #{steps: 6}));
}
fn fast(me) {
    this.verify_delta(me, 6, 6, 6);
    this.sample(me);
    require(cmd("SetTimeTransport", #{playing: true, rate: 4.0}));
    require(cmd("StepSimulation", #{hold: false}));
}
fn hold(me) { require(cmd("StepSimulation", #{hold: true})); }
fn verify_automatic(me) {
    let clock = clock_snapshot();
    let ticks = clock.sim_tick - this.clock.sim_tick;
    let elapsed = ticks * clock.simulation_dt_s;
    this.checks += [
        t_true(ticks > 0, "automatic transport admits real simulation ticks"),
        t_true(clock.simulation_dt_s == this.clock.simulation_dt_s,
            "speed changes preserve the canonical physics quantum"),
        t_true(abs(get(this.decay, "FmuParticipant.time_s") - this.decay_time - elapsed) <= 6.0 * clock.simulation_dt_s + 1.0e-12,
            "the slower external solver follows automatic physics demand within its communication interval"),
        t_true(abs(get(this.bounce, "FmuParticipant.time_s") - this.bounce_time - elapsed) <= 3.0 * clock.simulation_dt_s + 1.0e-12,
            "the event-driven solver follows automatic physics demand within its communication interval"),
        t_true(abs(clock.physics_elapsed_s - this.clock.physics_elapsed_s - ticks * clock.fixed_dt_s) < 1.0e-12,
            "automatic transport preserves every native physics tick")
    ];
}
fn slow(me) {
    this.verify_automatic(me);
    this.sample(me);
    require(cmd("SetTimeTransport", #{playing: true, rate: 0.5}));
    require(cmd("StepSimulation", #{hold: false}));
}
fn retire(me) {
    this.verify_automatic(me);
    require(cmd("ClearScene", #{}));
}
fn retired() {
    query("GetReadiness").ready && !clock_snapshot().simulation_progress.held
        && find("FMI Clock Decay") < 0 && find("FMI Clock Bounce") < 0
}
fn finish(me) {
    this.checks.push(t_true(retired(), "scene teardown retires native participants and their progress holds"));
    this.done = true;
    report_verdict(this.checks, "FMI SHARED CLOCK", "FMI_SHARED_CLOCK");
}
fn task(me, ctx) {
    seq([
        wait_until(|m| settled()), once(Fn("admitted")),
        wait_until(|m| settled()), once(Fn("bounced")), wait(0.05), once(Fn("frozen")),
        wait_until(|m| settled()), once(Fn("locally_paused")),
        wait_until(|m| settled()), once(Fn("fast")), wait(0.15), once(Fn("hold")),
        wait_until(|m| settled()), once(Fn("slow")), wait(0.15), once(Fn("hold")),
        wait_until(|m| settled()), once(Fn("retire")),
        wait_until(|m| retired()), once(Fn("finish"))
    ])
}
