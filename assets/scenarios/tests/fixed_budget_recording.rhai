// TEST-ONLY. Run in a rendered production session with a writable output_dir.
// A 25 FPS frame at 64x admits more than 64 fixed cycles; the recorder must
// finish draining that frame's duration before requesting its readback.
const TEST_KIND = "editor";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["GetOfflineRecordingStatus", "SimulationTimingProfile"],
       required_inputs: [] }
}

fn on_start(me, ctx) {
    if ctx.output_dir == () || ctx.output_dir == "" {
        throw "a writable recording output_dir is required";
    }
    this.ticks = 0;
    this.done = false;
    this.saw_pending = false;
    this.saw_budget = false;
    this.partial_capture = false;
    this.budget_exceeded = false;
    cmd("SetTimeTransport", #{ playing: true, rate: 64.0 });
    cmd("StartOfflineRecording", #{ output_dir: ctx.output_dir, fps: 25 });
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks % 8 != 0 { return; }
    let recording = query("GetOfflineRecordingStatus");
    let timing = query("SimulationTimingProfile").fixed_loop;
    let pending = timing.latest_pending_simulation_secs;
    this.saw_pending = this.saw_pending || pending >= dt();
    this.saw_budget = this.saw_budget || timing.latest_steps.to_float() == 64.0;
    this.budget_exceeded = this.budget_exceeded || timing.latest_steps.to_float() > 64.0;
    this.partial_capture = this.partial_capture
        || (recording.is_waiting_for_frame && pending >= dt());
    if recording.frame_index.to_float() < 3.0 && this.ticks < 1800
        && !this.partial_capture && !this.budget_exceeded { return; }
    this.done = true;
    cmd("StopOfflineRecording", #{});
    cmd("SetTimeTransport", #{ playing: true, rate: 1.0 });
    report_verdict([
        t_true(recording.active && recording.frame_index.to_float() >= 3.0,
            "three frames completed through production GPU readback"),
        t_true(this.saw_pending, "whole fixed cycles remain queued after a budgeted update"),
        t_true(this.saw_budget, "the production fixed loop reaches its 64-cycle budget"),
        t_true(!this.budget_exceeded, "no update exceeds the fixed-cycle budget"),
        t_true(!this.partial_capture, "readback never starts with whole fixed cycles pending"),
        t_true(timing.max_delta_limited_simulation_secs_total == 0.0,
            "recorded simulation demand is not clipped"),
    ], "FIXED BUDGET RECORDING", "FIXED_BUDGET_RECORDING");
}
