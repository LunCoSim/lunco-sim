// Attach through RunScenarioAsset to a settled celestial DEM scene with a
// wired environment probe, passing its stable probe_id in params.
const DURATION_S = 40.0;

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [ctx.probe_id], entity_writes: [],
       query_reads: [], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.probe = ctx.probe_id;
    this.started = false;
    this.done = false;
    this.samples = 0;
    this.valid = true;
    this.physics_conserved = true;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    let clock = clock_snapshot();
    if !this.started {
        this.before = clock;
        let result = cmd("SetCelestialClock", #{scale: 100000.0});
        this.accepted = result != () && result.ok == true;
        this.started = true;
        return;
    }
    let visibility = get(this.probe, "sun_visibility");
    let irradiance = get(this.probe, "sun_irradiance_w_m2");
    this.valid = this.valid && visibility != () && irradiance != ()
        && visibility >= 0.0 && visibility <= 1.0 && irradiance >= 0.0;
    this.physics_conserved = this.physics_conserved
        && abs((clock.physics_elapsed_s - this.before.physics_elapsed_s)
            - (clock.sim_tick - this.before.sim_tick) * clock.fixed_dt_s) < 1.0e-9
        && !clock.physics_paused && clock.physics_hold_reasons.len() == 0;
    this.samples += 1;
    if clock.world_sim_s - this.before.world_sim_s < DURATION_S { return; }

    this.done = true;
    cmd("SetCelestialClock", #{scale: this.before.celestial_rate});
    report_verdict([
        t_true(this.accepted && clock.celestial_rate == 100000.0,
            "the maximum celestial rate remains installed"),
        t_true(clock.celestial_epoch_jd - this.before.celestial_epoch_jd > 40.0,
            "the probe traversed more than a lunar illumination cycle"),
        t_true(clock.transport_rate == this.before.transport_rate,
            "celestial acceleration preserves the physical transport rate"),
        t_true(this.samples >= 2400 && clock.sim_tick > this.before.sim_tick,
            "fixed physics continued through the accelerated celestial window"),
        t_true(this.physics_conserved,
            "physics elapsed time matches every completed fixed step"),
        t_true(this.valid,
            "every demanded physical solar sample remained available and bounded"),
    ], "CELESTIAL TERRAIN CLOCK", "CELESTIAL_TERRAIN_CLOCK");
}
