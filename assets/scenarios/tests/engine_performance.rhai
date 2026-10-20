// @timing presentation

// Runs in both production hosts; the driver declares whether frames are rendered.
fn on_start(me, ctx) {
    this.done = false;
    this.ticks = 0;
    this.rendered = ctx.rendered;
    this.initial = query("EnginePerformance");
    this.clock = clock_snapshot();
    let paused = cmd("SetTimeTransport", #{playing: false});
    if paused.ok != true { throw paused.to_string(); }
    this.paused_clock = ();
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    let clock = clock_snapshot();
    if clock.transport_playing { return; }
    if this.paused_clock == () { this.paused_clock = clock; }
    let sample = query("EnginePerformance");
    if this.rendered
        && sample.frame_sample_count - this.initial.frame_sample_count <= sample.history_capacity {
        if this.ticks > 3000 { throw "frame history did not advance while paused"; }
        return;
    }
    let checks = [
        t_true(sample.available == this.rendered, "availability reports actual rendered diagnostics"),
        t_true(sample.frame_time_samples_ms.len() <= sample.history_capacity,
            "the canonical frame history remains bounded"),
        t_true(clock.sim_tick == this.paused_clock.sim_tick,
            "reading render performance does not advance paused simulation ticks")
    ];
    if this.rendered {
        let valid = true;
        for value in sample.frame_time_samples_ms {
            if !(value >= 0.0 && value < 1.0e12) { valid = false; }
        }
        checks += [
            t_true(sample.frame_time_samples_ms.len() == sample.history_capacity,
                "render frames fill and roll the canonical retained history"),
            t_true(sample.frame_sample_count > this.initial.frame_sample_count,
                "application sample identity advances independently of paused simulation"),
            t_true(valid && sample.raw_frame_time_ms != () && sample.fps != (),
                "rendered measurements retain numeric frame diagnostics")
        ];
    } else {
        checks += [
            t_true(sample.frame_time_samples_ms.len() == 0,
                "a headless host does not fabricate rendered frame samples"),
            t_true(sample.frame_sample_count == 0,
                "unavailable frame diagnostics do not advance sample identity"),
            t_true(sample.raw_frame_time_ms == () && sample.fps == (),
                "unavailable diagnostics remain explicit missing values")
        ];
    }
    this.done = true;
    cmd("SetTimeTransport", #{playing: this.clock.transport_playing});
    report_verdict(checks, "ENGINE PERFORMANCE", "ENGINE_PERFORMANCE");
}
