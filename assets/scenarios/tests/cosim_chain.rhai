// Production USD/Rhai acceptance for Modelica -> Python -> Avian propagation.
//
// USD authors the participants and native connections. This scenario observes
// only the public runtime surfaces, so the same contract covers the scene
// loader, source dispatch, cross-entity wiring, Python backend, and Avian sink.

const WAIT_S = 30.0;

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
    this.osc = find("/CosimChain/Oscillator");
    this.amp = find("/CosimChain/Amplifier");
    this.target = find("/CosimChain/CosimTarget");
    this.max_signal = 0.0;
    this.max_scaled = 0.0;
    this.max_velocity = 0.0;
}

fn fail(message) {
    this.done = true;
    fail_fast(message, "COSIM CHAIN", "COSIM_CHAIN");
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    if this.osc < 0 || this.amp < 0 || this.target < 0 {
        fail("required cosim-chain participant did not compose");
        return;
    }

    let signal = get(this.osc, "signal");
    let scaled = get(this.amp, "scaled");
    let velocity = get(this.target, "velocity_y");
    if signal != () && signal.abs() > this.max_signal.abs() {
        this.max_signal = signal;
    }
    if scaled != () && scaled.abs() > this.max_scaled.abs() {
        this.max_scaled = scaled;
    }
    if velocity != () && velocity.abs() > this.max_velocity.abs() {
        this.max_velocity = velocity;
    }

    if this.elapsed < 8.0 { return; }

    let failures = [];
    failures.push(t_true(this.max_signal.abs() > 0.5,
        "Modelica oscillator did not publish a signal near its authored amplitude"));
    failures.push(t_true(this.max_scaled.abs() > 25.0,
        "Python amplifier did not publish the expected gain-scaled signal"));
    failures.push(t_true(this.max_velocity.abs() > 0.5,
        "Avian rigid body did not integrate the amplified force"));
    print("[cosim-chain] max signal=" + r4(this.max_signal) +
          " scaled=" + r4(this.max_scaled) +
          " velocity_y=" + r4(this.max_velocity));
    this.done = true;
    report_verdict(failures, "COSIM CHAIN", "COSIM_CHAIN");
}
