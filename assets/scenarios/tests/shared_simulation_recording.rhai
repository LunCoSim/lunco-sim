// @timing presentation
fn probe() {
    import "lunco://scenarios/tests/modelica_initialization_network.rhai" as network;
    network::row(query("CosimStatus").entities, "ClockProbe")
}
fn on_start(me, ctx) {
    subscribe("SHARED_RECORDING_FRAME");
    this.failures = [];
    import "lunco://scripting/lib/shots.rhai" as shots;
    let invalid_rejected = false;
    try { shots::shot(#{out: "target/shared-simulation-recording", idx: 0, frames: 0}); }
    catch (error) { invalid_rejected = true; }
    this.failures.push(t_true(invalid_rejected, "zero-frame shots reject before arming a recorder"));

    this.callbacks = [];
    cmd("StepSimulation", #{hold: true});
}
fn baseline(me) {
    this.start = clock_snapshot();
    this.model_start = probe().modelica_current_time;
    this.callbacks = [];
}
fn capture_frame(me, index) {
    let clock = clock_snapshot();
    let observation = #{index: index, tick: clock.sim_tick,
        model_time: probe().modelica_current_time, quantum: clock.simulation_dt_s};
    if index > 0 {
        observation.capture_tick = query("GetOfflineRecordingStatus").last_capture_sim_tick.to_int();
    }
    emit("SHARED_RECORDING_FRAME", observation);
    shot_step();
}
fn on_event(me, event, ctx) {
    if event.name != "SHARED_RECORDING_FRAME" { return; }
    let observation = event.value;
    this.failures.push(t_true(observation.tick - this.start.sim_tick == observation.index,
        "each image callback follows the preceding admitted cycle"));
    if observation.index > 0 {
        this.failures.push(t_true(observation.capture_tick - this.start.sim_tick == observation.index,
            "the saved image was requested after its physics cycle"));
        this.failures.push(t_true(abs(observation.model_time - this.model_start
            - observation.index * observation.quantum) < 1.0e-8,
            "capture admission waits for the Modelica receipt"));
    }
    this.callbacks.push(observation.index);
}
fn check_take(me, count) {
    let clock = clock_snapshot();
    let receipt = query("GetOfflineRecordingStatus");
    this.failures += [
        t_true(this.callbacks.len == count, "the take has exactly one callback per image"),
        t_true(receipt.frame_index.to_int() == count && !receipt.active,
            "the shot saves exactly the requested image count and closes"),
        t_true(receipt.last_capture_sim_tick.to_int() - this.start.sim_tick == count,
            "the final image includes its final admitted cycle"),
        t_true(clock.sim_tick - this.start.sim_tick == count,
            "the recorder neither duplicates nor skips physical cycles"),
        t_true(abs(probe().modelica_current_time - this.model_start - count * clock.simulation_dt_s) < 1.0e-13,
            "the completed take retains the coupled Modelica coordinate")
    ];
}
fn four_complete(me) { this.check_take(me, 4); this.baseline(me); }
fn one_complete(me) { this.check_take(me, 1); this.baseline(me); }
fn release(me) {
    let clock = clock_snapshot();
    let receipt = query("GetOfflineRecordingStatus");
    this.failures += [
        t_true(receipt.frame_index.to_int() == 2, "a motionless shot completes both images"),
        t_true(clock.sim_tick == this.start.sim_tick && probe().modelica_current_time == this.model_start,
            "a motionless shot freezes physics, Modelica and simulation timers together")
    ];
    cmd("StepSimulation", #{hold: false});
    this.resume_tick = clock.sim_tick;
}
fn finished(me) {
    this.failures.push(t_true(clock_snapshot().sim_tick > this.resume_tick,
        "closing the final take restores realtime simulation and presentation waits"));
    report_verdict(this.failures, "SHARED SIMULATION RECORDING", "SHARED_SIMULATION_RECORDING");
}
fn task(me, ctx) {
    import "lunco://scripting/lib/shots.rhai" as shots;
    seq([
        wait_until(|m| !clock_snapshot().barrier_held), once(Fn("baseline")),
        shots::shot(#{out: "target/shared-simulation-recording", idx: 0,
            fps: 30, frames: 4, frozen: true, frame: Fn("capture_frame")}),
        once(Fn("four_complete")),
        shots::shot(#{out: "target/shared-simulation-recording", idx: 1,
            fps: 30, frames: 1, frozen: true, frame: Fn("capture_frame")}),
        once(Fn("one_complete")),
        shots::shot(#{out: "target/shared-simulation-recording", idx: 2,
            fps: 30, frames: 2, frozen: true}),
        once(Fn("release")), wait(0.1), once(Fn("finished"))
    ])
}
