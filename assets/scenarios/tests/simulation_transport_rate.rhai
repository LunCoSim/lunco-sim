// @timing presentation
// Windowed acceptance: measure completed time under ordinary host pacing.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: [], required_inputs: []}
}

fn rate_matches(actual, requested) {
    actual >= requested * 0.99 && actual <= requested * 1.01
}

fn begin_rate(rate) {
    let result = cmd("SetTimeTransport", #{playing: true, rate: rate});
    if result.ok != true { throw result.to_string(); }
    this.warming = true;
    this.warmup_started = clock_snapshot().wall_elapsed_s;
    print("TRANSPORT_RATE_WARMUP requested=" + rate.to_string());
}

fn begin_sample(rate) {
    let clock = clock_snapshot();
    this.warming = false;
    this.start_tick = clock.sim_tick;
    this.start_wall = clock.wall_elapsed_s;
    this.start_physics = clock.physics_elapsed_s;
    this.native_dt = clock.fixed_dt_s;
    print("TRANSPORT_RATE_BEGIN requested=" + rate.to_string());
}

fn on_start(me, params) {
    this.rates = params.rates;
    if this.rates.is_empty() { throw "at least one transport rate is required"; }
    this.sample_seconds = params.sample_seconds;
    if this.sample_seconds < 3.0 || this.sample_seconds > 60.0 {
        throw "transport samples require three to 60 seconds";
    }
    this.warmup_seconds = params.warmup_seconds;
    if this.warmup_seconds < 0.0 || this.warmup_seconds > 10.0 {
        throw "transport warmup requires zero to ten seconds";
    }
    this.index = 0;
    this.done = false;
    this.checks = [
        t_true(clock_snapshot().barrier_active_participants == params.participants,
            "the admitted participant set does not match the requested scene"),
        t_true(!rate_matches(1.0, 2.0),
            "negative fixture accepted realtime execution for a 2x request")
    ];
    this.begin_rate(this.rates[0]);
}

fn on_tick(me, ctx) {
    if this.done { return; }
    let clock = clock_snapshot();
    if this.warming {
        if clock.wall_elapsed_s - this.warmup_started >= this.warmup_seconds {
            this.begin_sample(this.rates[this.index]);
        }
        return;
    }
    let wall = clock.wall_elapsed_s - this.start_wall;
    if wall < this.sample_seconds { return; }
    let requested = this.rates[this.index];
    let completed_ticks = (clock.sim_tick - this.start_tick).to_float();
    let actual = completed_ticks * clock.simulation_dt_s / wall;
    let native_elapsed = clock.physics_elapsed_s - this.start_physics;
    print("TRANSPORT_RATE_SAMPLE requested=" + requested.to_string()
        + " actual=" + actual.to_string() + " wall_s=" + wall.to_string());
    this.checks.push(t_true(rate_matches(actual, requested),
        "requested " + requested.to_string() + "x, completed " + actual.to_string() + "x"));
    this.checks.push(t_max(native_elapsed - completed_ticks * this.native_dt,
        this.native_dt * 0.001,
        "native physics time error at " + requested.to_string() + "x"));
    this.index += 1;
    if this.index < this.rates.len() {
        this.begin_rate(this.rates[this.index]);
    } else {
        this.done = true;
        cmd("SetTimeTransport", #{playing: false, rate: 1.0});
        report_verdict(this.checks, "SIMULATION TRANSPORT RATE", "SIMULATION_TRANSPORT_RATE");
    }
}
