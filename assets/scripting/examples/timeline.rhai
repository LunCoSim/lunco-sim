// Example: a mission as DATA (Layer 2). Unlike sequence.rhai (which carries
// rhai closures), every step here is a plain map of fields — so this whole
// const is serialisable. The exact same data can be sent over the API as
// `RunTimeline { target, timeline }` with NO rhai authored at all; this file
// just shows the equivalent in-scenario form.
//
// compile_timeline lowers the data steps to Layer-1 steps once when `task(me)`
// is initialized; the native task kernel owns fixed-tick progression and event
// delivery.

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

fn task(me) {
    print("timeline '" + TIMELINE.name + "': " + TIMELINE.steps.len() + " steps queued");
    seq(compile_timeline(TIMELINE.steps))
}
