// Modelica SIX-INDEPENDENT drive-law BRIDGE — the actuation half of the
// `driveLaw = "modelica"` variant
// (`components/mobility/drive_laws/modelica_six_independent.usda`).
//
// The PHYSICS lives in `models/RoverDrivetrain.mo` — the SAME per-side motor
// lag the skid law uses, deliberately: the law is per-SIDE (one motor
// controller per bank), and fanning one side out to three wheel ports is
// WIRING, not physics. So this script reads the sibling model's
// `drive_left`/`drive_right` outputs and writes the rover's six per-wheel FSW
// ports (`six_wheel_independent.usda` declares them via `lunco:drivePorts`):
// left bank w0/w1/w2 = left, right bank w3/w4/w5 = right — the same banks the
// authored `lunco:driveMix` feeds, which stands down via the variant's
// `lunco:driveKernel = "external"` sentinel, so this bridge is the ONLY
// writer of those ports.
//
// Mounted as `def LunCoProgram "DriveBridge"` on the rover root, so `me` IS
// the rover — sibling lookup by published output name, no name inference
// (see rover_modelica_drive.rhai for the two-port skid sibling of this file).

fn sibling_out(me, name) {
    for c in children(me) {
        let v = get(c, name);
        if v != () { return v; }
    }
    0.0
}

fn on_tick(me) {
    let l = sibling_out(me, "drive_left");
    let r = sibling_out(me, "drive_right");
    set(me, "drive_w0", l);
    set(me, "drive_w1", l);
    set(me, "drive_w2", l);
    set(me, "drive_w3", r);
    set(me, "drive_w4", r);
    set(me, "drive_w5", r);
}
