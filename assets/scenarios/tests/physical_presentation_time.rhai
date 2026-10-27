// @timing presentation
// Test-only observer of the production presentation clock and transport commands.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: [], required_inputs: []}
}
fn forward_sample(previous, current, completed) {
    current >= previous && current <= completed
}
fn on_start(me, ctx) {
    this.phase = 0;
    this.frames = 0;
    this.previous = ();
    this.errors = [];
    this.fractional = 0;
    this.substep = ();
    this.substep_changed = false;
    this.done = false;
    this.initial = clock_snapshot();
    this.checks = [
        t_true(!forward_sample(10.0, 9.5, 10.0), "negative fixture rejects rewind"),
        t_true(!forward_sample(10.0, 11.0, 10.0), "negative fixture rejects extrapolation")
    ];
}
fn issue(name, values) {
    let result = cmd(name, values);
    if result.ok != true { throw(result.to_string()); }
}
fn on_tick(me, ctx) {
    if this.done { return; }
    let clock = clock_snapshot();
    let sample = get_setting("SimulationPresentationTime.sim_tick");
    let alpha = get_setting("SimulationPresentationTime.interpolation");
    // Native zero delta means no physics admission; compare actual integrated quanta.
    if clock.physics_substep_dt_s > 0.0 {
        if this.substep == () { this.substep = clock.physics_substep_dt_s; }
        else if this.substep != clock.physics_substep_dt_s { this.substep_changed = true; }
    }
    if this.previous != () && !forward_sample(this.previous, sample, clock.sim_tick.to_float()) {
        if this.errors.is_empty() { this.errors.push([this.previous, sample, clock.sim_tick]); }
    }
    this.previous = sample;
    if alpha > 0.0 && alpha < 1.0 { this.fractional += 1; }
    this.frames += 1;
    if this.phase == 0 && this.frames >= 120 {
        this.checks.push(t_true(this.fractional > 0, "running presentation exercises fractional intervals"));
        issue("SetTimeTransport", #{playing: false});
        this.phase = 1; this.frames = 0;
    } else if this.phase == 1 && this.frames >= 12 {
        this.held = clock;
        this.checks.push(t_true(alpha == 1.0 && sample == clock.sim_tick,
            "pause displays the completed endpoint"));
        issue("StepSimulation", #{hold: true, steps: 1});
        this.phase = 2; this.frames = 0;
    } else if this.phase == 2 && this.frames >= 12 {
        this.checks.push(t_true(clock.sim_tick == this.held.sim_tick + 1
            && sample == clock.sim_tick && alpha == 1.0,
            "one manual step displays exactly one new completed endpoint"));
        issue("StepSimulation", #{hold: false});
        issue("SetTimeTransport", #{playing: true, rate: 0.25});
        this.slow_start = clock.sim_tick;
        this.phase = 3; this.frames = 0;
    } else if this.phase == 3 && this.frames >= 120 {
        this.checks.push(t_true(clock.sim_tick > this.slow_start && clock.transport_rate == 0.25,
            "fractional rate advances the same fixed clock"));
        issue("SetTimeTransport", #{rate: 8.0});
        this.fast_start = clock.sim_tick;
        this.phase = 4; this.frames = 0;
    } else if this.phase == 4 && this.frames >= 30 {
        this.checks.push(t_true(clock.sim_tick > this.fast_start && clock.transport_rate == 8.0,
            "accelerated transport advances completed fixed intervals"));
        this.checks.push(t_true(clock.fixed_dt_s == this.initial.fixed_dt_s
            && this.substep != () && !this.substep_changed,
            "transport rates preserve fixed physics and substep quanta"));
        issue("SetTimeTransport", #{playing: false, rate: 1.0});
        this.phase = 5; this.frames = 0;
    } else if this.phase == 5 && this.frames >= 12 {
        this.epoch_tick = clock.sim_tick;
        this.epoch_native = clock.physics_elapsed_s;
        issue("SetMissionEpoch", #{epoch_jd: 2451546.0});
        this.phase = 6; this.frames = 0;
    } else if this.phase == 6 && this.frames >= 12 {
        this.checks.push(t_true(clock.sim_tick == this.epoch_tick
            && clock.physics_elapsed_s == this.epoch_native
            && clock.presentation_sim_s == 0.0
            && clock.presentation_epoch_jd == 2451546.0,
            "epoch reset remaps presentation without advancing native time"));
        this.celestial_paused = clock;
        issue("SetCelestialClock", #{scale: 100000.0});
        this.phase = 7; this.frames = 0;
    } else if this.phase == 7 && this.frames >= 12 {
        this.checks.push(t_true(clock.celestial_rate == 100000.0
            && clock.celestial_parent == "world_time"
            && clock.celestial_epoch_jd == this.celestial_paused.celestial_epoch_jd
            && clock.sim_tick == this.celestial_paused.sim_tick,
            "accelerated celestial clock remains held with its physical parent"));
        this.celestial_start = clock;
        issue("SetTimeTransport", #{playing: true});
        this.phase = 8; this.frames = 0;
    } else if this.phase == 8 && this.frames >= 120 {
        let celestial_seconds = (clock.celestial_epoch_jd - this.celestial_start.celestial_epoch_jd) * 86400.0;
        let physical_seconds = clock.world_sim_s - this.celestial_start.world_sim_s;
        this.checks.push(t_true(physical_seconds > 0.0
            && (celestial_seconds - physical_seconds * 100000.0).abs() < 0.001
            && clock.transport_rate == 1.0 && clock.fixed_dt_s == this.initial.fixed_dt_s,
            "celestial acceleration scales its own clock without scaling physical cadence"));
        issue("SetTimeTransport", #{playing: false});
        this.phase = 9; this.frames = 0;
    } else if this.phase == 9 && this.frames >= 12 {
        this.celestial_step = clock;
        issue("StepSimulation", #{hold: true, steps: 1});
        this.phase = 10; this.frames = 0;
    } else if this.phase == 10 && this.frames >= 12 {
        let celestial_seconds = (clock.celestial_epoch_jd - this.celestial_step.celestial_epoch_jd) * 86400.0;
        this.checks.push(t_true(clock.sim_tick == this.celestial_step.sim_tick + 1
            && (celestial_seconds - 100000.0 / 60.0).abs() < 0.001,
            "one physical step advances the accelerated celestial child by its exact mapped interval"));
        this.checks.push(t_true(this.errors.is_empty(),
            "all displayed intervals stay monotonic and bounded: " + this.errors.to_string()));
        this.done = true;
        report_verdict(this.checks, "PHYSICAL PRESENTATION TIME", "PHYSICAL_PRESENTATION_TIME");
    }
}
