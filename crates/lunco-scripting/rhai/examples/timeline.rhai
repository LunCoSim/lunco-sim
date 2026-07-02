// Example: a mission as DATA (Layer 2). Unlike sequence.rhai (which carries
// rhai closures), every step here is a plain map of fields — so this whole
// const is serialisable. The exact same data can be sent over the API as
// `RunTimeline { target, timeline }` with NO rhai authored at all; this file
// just shows the equivalent in-scenario form.
//
// compile_timeline lowers the data steps to Layer-1 steps once in on_start; the
// generic three-hook executor below is what RunTimeline injects automatically.

const TIMELINE = #{
    name: "survey",
    steps: [
        #{ move_to: [12.0, 0.0,  0.0], speed: 1.0, radius: 2.0 }, // drive to A
        #{ wait: 5.0 },                                           // dwell / sample
        #{ emit: "SAMPLE_DONE", value: true },                    // mark progress
        #{ wait_event: "GO" },                                    // hold for a hail
        #{ move_to: [12.0, 0.0, 25.0], speed: 1.0, radius: 2.0 }, // drive to B
        #{ cmd: "SetPorts", params: #{ writes: [["brake", 1.0]] } }, // stop
    ],
};

fn on_start(me) {
    this.cur = seq_init();
    this.steps = compile_timeline(TIMELINE.steps);
    print("timeline '" + TIMELINE.name + "': " + this.steps.len() + " steps queued");
}

fn on_tick(me) {
    this.cur = run_steps(me, this.steps, this.cur);
}

fn on_event(me, evt) {
    this.cur = seq_note_event(this.cur, evt);
}
