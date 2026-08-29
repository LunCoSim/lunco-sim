// ── Production lander RCS mount and wrench test ─────────────────────────────
//
// This test owns the authored arrangement contract. Rust still owns USD
// decoding, body-frame conversion, and Avian force application; Rhai checks
// what the composed asset exposes and what the production projection emitted.

const ROOT = "/LanderRcsTest/Lander";
const ACTUATION = ROOT + "/AttitudeActuation";
const EPS = 0.0001;

fn close_vec(actual, expected, tolerance) {
    if actual == () || expected == () || actual.len() != expected.len() {
        return false;
    }
    for i in 0..actual.len() {
        if (actual[i] - expected[i]).abs() > tolerance {
            return false;
        }
    }
    true
}

fn mount_ok(path, position, rotation, direction) {
    let record = query("QueryUsdPrim", #{
        path: path,
        attrs: [
            "xformOp:translate",
            "xformOp:rotateXYZ",
            "lunco:forceActuator:direction",
        ],
    });
    if record == () || record.ok == false || record.attrs == () {
        return false;
    }
    close_vec(record.attrs["xformOp:translate"], position, EPS)
        && close_vec(record.attrs["xformOp:rotateXYZ"], rotation, EPS)
        && close_vec(record.attrs["lunco:forceActuator:direction"], direction, EPS)
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
    this.mounts = [
        ["RcsPitchPos", [0.0, 0.0, 2.4], [0.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
        ["RcsPitchNeg", [0.0, 0.0, -2.4], [0.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
        ["RcsRollPos", [2.4, 0.0, 0.0], [180.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
        ["RcsRollNeg", [-2.4, 0.0, 0.0], [180.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
        ["RcsYawPos", [2.4, 0.0, 0.0], [90.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
        ["RcsYawNeg", [-2.4, 0.0, 0.0], [90.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
        ["RcsPitchPosB", [0.0, 0.0, -2.4], [180.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
        ["RcsPitchNegB", [0.0, 0.0, 2.4], [180.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
        ["RcsRollPosB", [-2.4, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
        ["RcsRollNegB", [2.4, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
        ["RcsYawPosB", [-2.4, 0.0, 0.0], [-90.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
        ["RcsYawNegB", [2.4, 0.0, 0.0], [-90.0, 0.0, 0.0], [0.0, -1.0, 0.0]],
    ];
    this.quads = [
        ["RcsQuadPXPZ", [1.8, 0.0, 1.8]],
        ["RcsQuadNXPZ", [-1.8, 0.0, 1.8]],
        ["RcsQuadPXNZ", [1.8, 0.0, -1.8]],
        ["RcsQuadNXNZ", [-1.8, 0.0, -1.8]],
    ];
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    // The source is published only after the USD projection has admitted the
    // composed collection. Keep waiting for that authoritative artifact rather
    // than declaring a static-only pass.
    let generated = query("GeneratedModelicaSource", #{ network_root: ACTUATION });
    if generated == () || generated.networks == () || generated.networks.len() != 1
        || generated.networks[0].source == () {
        if this.elapsed > 30.0 {
            this.done = true;
            report_verdict(["the production RCS allocator source was not published"],
                           "LANDER RCS", "LANDER_RCS");
        }
        return;
    }

    let failures = [];
    for spec in this.mounts {
        let path = ROOT + "/" + spec[0];
        failures.push(t_true(mount_ok(path, spec[1], spec[2], spec[3]),
                             path + " has an unexpected authored mount or force direction"));
    }
    for spec in this.quads {
        let path = ROOT + "/" + spec[0];
        let record = query("QueryUsdPrim", #{ path: path, attrs: ["xformOp:translate"] });
        let ok = record != () && record.ok != false && record.attrs != ()
            && close_vec(record.attrs["xformOp:translate"], spec[1], EPS);
        failures.push(t_true(ok, path + " is not seated at its diagonal hardpoint"));
    }

    // In the six-row body wrench, the four yaw columns are columns 9..12.
    // With yaw mounts in the y=0 body plane, their row-4 (pitch) contribution
    // is zero. This pins the actual generated geometry/torque matrix, not just
    // the source attributes. The string is stable generated Modelica output;
    // the authored pose checks above provide the readable per-prim diagnostics.
    let source = generated.networks[0].source;
    let yaw_pitch_free = source.contains(
        "3000.0001192092896, 3000.0001192092896, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0; 0.0");
    failures.push(t_true(yaw_pitch_free,
                         "generated yaw jets still contribute a pitch moment"));

    this.done = true;
    report_verdict(failures, "LANDER RCS", "LANDER_RCS");
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        report_verdict(["RCS arrangement test stopped before the production source was observed"],
                       "LANDER RCS", "LANDER_RCS");
    }
}
