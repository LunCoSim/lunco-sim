// @timing presentation
// Application-owned retained observer: transport edges remain observable paused
// and after scene teardown. Temporary admission holds are not user pauses.
fn on_start(me, ctx) {
    this.done = false; this.phase = "initial"; this.waited = 0;
    this.deadline = clock_snapshot().wall_elapsed_s + 90.0;
    this.started = 0; this.paused = 0; this.unpaused = 0; this.stopped = 0;
    this.checks = [];
    for name in ["SIMULATION_STARTED", "SIMULATION_PAUSED", "SIMULATION_UNPAUSED", "SIMULATION_STOPPED"] {
        subscribe(name);
    }
}
fn on_event(me, evt, ctx) {
    if evt.name == "SIMULATION_STARTED" { this.started += 1; }
    if evt.name == "SIMULATION_PAUSED" { this.paused += 1; }
    if evt.name == "SIMULATION_UNPAUSED" { this.unpaused += 1; }
    if evt.name == "SIMULATION_STOPPED" { this.stopped += 1; }
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.waited += 1;
    if clock_snapshot().wall_elapsed_s > this.deadline { throw "simulation lifecycle timed out at " + this.phase; }
    // This observer may start while the fresh host's startup scene is pending.
    // A presentation frame count cannot establish an admitted simulation tick.
    if this.phase == "initial" && clock_snapshot().sim_tick > 0 {
        cmd("SetTimeTransport", #{rate: 8.0});
        this.phase = "initial_fast"; this.waited = 0; return;
    }
    if this.phase == "initial_fast" && clock_snapshot().virtual_rate == 8.0 {
        cmd("ClearScene", #{});
        this.phase = "initial_clear"; this.waited = 0; return;
    }
    if this.phase == "initial_clear" && this.stopped == 1 {
        this.started = 0; this.paused = 0; this.unpaused = 0; this.stopped = 0;
        this.phase = "empty"; this.waited = 0; return;
    }
    if this.phase == "empty" && this.waited >= 6 {
        this.checks.push(t_true(this.started == 0 && this.stopped == 0,
            "an empty scene does not fabricate simulation lifecycle edges"));
        let timing = query("SimulationTimingProfile").fixed_loop;
        this.checks.push(t_true(clock_snapshot().transport_rate == 1.0 && clock_snapshot().virtual_rate == 1.0
            && timing.latest_pending_simulation_secs == 0.0,
            "clearing the fast scene resets both rates and retires all pending simulation time"));
        let a = cmd("LoadScene", #{path: ctx.scene});
        if a.ok != true { throw a.to_string(); }
        this.phase = "started"; this.waited = 0; return;
    }
    if this.phase == "started" && this.started == 1 {
        this.checks.push(t_true(query("GetReadiness").ready && clock_snapshot().transport_playing,
            "started follows actual scene admission"));
        this.checks.push(t_true(this.paused == 0 && this.unpaused == 0,
            "scene loading holds do not create pause or unpause events"));
        this.checks.push(t_true(clock_snapshot().transport_rate == 1.0 && clock_snapshot().virtual_rate == 1.0,
            "the loaded scene starts at realtime"));
        cmd("SetTimeTransport", #{playing: false});
        cmd("SetTimeTransport", #{playing: false});
        this.pause_tick = clock_snapshot().sim_tick;
        let rejected = cmd("SetTimeTransport", #{playing: true, rate: 1000.0});
        this.checks.push(t_true(rejected.ok == false && !clock_snapshot().transport_playing,
            "a rejected combined transport request preserves the paused state"));
        this.phase = "paused"; this.waited = 0; return;
    }
    if this.phase == "paused" && this.waited >= 6 {
        this.checks.push(t_true(this.paused == 1 && this.unpaused == 0 && this.started == 1,
            "one accepted pause yields one event despite duplicate and rejected commands"));
        this.checks.push(t_true(clock_snapshot().sim_tick == this.pause_tick,
            "pause freezes the simulation while lifecycle delivery remains live"));
        cmd("SetTimeTransport", #{playing: true});
        cmd("SetTimeTransport", #{playing: true});
        this.phase = "unpaused"; this.waited = 0; return;
    }
    if this.phase == "unpaused" && this.unpaused == 1 && clock_snapshot().sim_tick > this.pause_tick {
        this.checks.push(t_true(this.started == 1 && this.paused == 1,
            "unpause resumes the existing simulation without another start"));
        cmd("StepSimulation", #{hold: true});
        this.hold_tick = clock_snapshot().sim_tick;
        this.phase = "held"; this.waited = 0; return;
    }
    if this.phase == "held" && this.waited >= 6 {
        this.checks.push(t_true(clock_snapshot().sim_tick == this.hold_tick && this.paused == 1,
            "a manual admission hold freezes ticks without a transport pause event"));
        cmd("StepSimulation", #{hold: false});
        this.phase = "released"; this.waited = 0; return;
    }
    if this.phase == "released" && clock_snapshot().sim_tick > this.hold_tick {
        this.checks.push(t_true(this.unpaused == 1 && this.started == 1,
            "releasing an admission hold does not fabricate an unpause or start"));
        cmd("SetTimeTransport", #{rate: 0.25});
        this.phase = "slow"; this.waited = 0; return;
    }
    if this.phase == "slow" && clock_snapshot().virtual_rate == 0.25 {
        let restart = cmd("RestartScene", #{reset_document: false});
        if restart.ok != true { throw restart.to_string(); }
        this.phase = "restarted"; this.waited = 0; return;
    }
    if this.phase == "restarted" && this.stopped == 1 && this.started == 2 {
        this.checks.push(t_true(clock_snapshot().transport_rate == 1.0 && clock_snapshot().virtual_rate == 1.0,
            "restarting the slow scene resets the requested and projected rate"));
        cmd("SetTimeTransport", #{rate: 4.0});
        this.phase = "switch_fast"; this.waited = 0; return;
    }
    if this.phase == "switch_fast" && clock_snapshot().virtual_rate == 4.0 {
        let replacement = cmd("OpenTwin", #{path: ctx.replacement_twin});
        if replacement.ok != true { throw replacement.to_string(); }
        this.phase = "switched"; this.waited = 0; return;
    }
    if this.phase == "switched" && this.stopped == 2 && this.started == 3 {
        let twin = query("ListTwin", #{limit: 0});
        this.checks.push(t_true(twin.root == ctx.replacement_twin && query("GetReadiness").ready
            && clock_snapshot().transport_rate == 1.0 && clock_snapshot().virtual_rate == 1.0,
            "switching to another Twin resets the requested and projected rate"));
        cmd("ClearScene", #{});
        this.phase = "stopped"; this.waited = 0; return;
    }
    if this.phase == "stopped" && this.stopped == 3 {
        this.checks.push(t_true(!clock_snapshot().transport_playing,
            "stopped is delivered after scene teardown closes the transport"));
        cmd("ClearScene", #{});
        this.phase = "empty_again"; this.waited = 0; return;
    }
    if this.phase == "empty_again" && this.waited >= 6 {
        this.checks.push(t_true(this.stopped == 3 && this.started == 3,
            "clearing an already empty scene produces no duplicate stop or start"));
        this.done = true;
        report_verdict(this.checks, "SIMULATION LIFECYCLE", "SIMULATION_LIFECYCLE");
    }
}
