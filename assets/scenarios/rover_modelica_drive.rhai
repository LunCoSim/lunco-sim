// Modelica drive-law BRIDGE — the actuation half of the `driveLaw = "modelica"`
// variant (`components/mobility/drive_laws/modelica_skid.usda`).
//
// The PHYSICS lives in `models/RoverDrivetrain.mo`: a first-order motor lag per
// side, integrating on the solver clock. This script owns NO dynamics — each
// tick it reads the sibling drivetrain model's normalized `drive_left`/
// `drive_right` outputs and writes them onto the rover's identically-named FSW
// drive ports, which the wheels are already wired to. The built-in skid mixing
// stands down via the variant's `lunco:driveKernel = "external"` sentinel, so
// this bridge is the ONLY writer of those ports.
//
// Mounted as `def LunCoProgram "DriveBridge"` on the rover root, so `me` IS the
// rover — the same sibling-lookup predicate style as leg_spring.rhai: "the
// child that publishes `drive_left`" is the whole lookup, no name inference.
//
// Removable: if native `inputs:<port>.connect` wiring onto FSW ports lands,
// the two connects replace this file.

fn sibling_out(me, name) {
    for c in children(me) {
        let v = get(c, name);
        if v != () { return v; }
    }
    0.0
}

fn on_tick(me) {
    set(me, "drive_left", sibling_out(me, "drive_left"));
    set(me, "drive_right", sibling_out(me, "drive_right"));
}
