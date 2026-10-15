// @timing presentation
// Run against the mesh-free modelica_initialization_network fixture.
fn on_start(me, ctx) {
    this.failures = [];
    this.observations = 0;
}
fn arm(me) {
    cmd("StartOfflineRecording", #{output_dir: "target/shared-recording-cancel", fps: 30});
}
fn observe_hold(me) {
    this.observations += 1;
    if this.observations == 2 { this.held_tick = clock_snapshot().sim_tick; }
    if this.observations == 10 {
        this.failures += [
            t_true(!query("GetOfflineRecordingStatus").active,
                "a mesh-free scene remains armed before visual readiness"),
            t_true(clock_snapshot().sim_tick == this.held_tick,
                "an armed visual wait admits no simulation time")
        ];
    }
}
fn observed(me) { this.observations >= 10 }
fn cancel(me) {
    cmd("StopOfflineRecording", #{});
    this.cancel_tick = clock_snapshot().sim_tick;
}
fn finished(me) {
    this.failures += [
        t_true(!query("GetOfflineRecordingStatus").active,
            "cancelling an armed request leaves no active recorder"),
        t_true(clock_snapshot().sim_tick > this.cancel_tick,
            "cancelling before readiness restores realtime simulation and timer progress")
    ];
    report_verdict(this.failures, "SHARED RECORDING CANCEL", "SHARED_RECORDING_CANCEL");
}
fn task(me, ctx) {
    seq([
        wait_until(|m| !clock_snapshot().barrier_held),
        once(Fn("arm")), step(Fn("observe_hold"), Fn("observed")),
        once(Fn("cancel")), wait(0.1), once(Fn("finished"))
    ])
}
