// Modelica ACKERMANN drive-law BRIDGE — the actuation half of the
// `driveLaw = "modelica"` variant
// (`components/mobility/drive_laws/modelica_ackermann.usda`).
//
// The PHYSICS lives in `models/RoverAckermannDrivetrain.mo`: one first-order
// motor lag on the shared axle, plus a `steering` passthrough. This script
// owns NO dynamics — each tick it reads the sibling drivetrain model's
// normalized outputs and writes them onto the rover's identically-named FSW
// ports. `steering` is included because the built-in Ackermann linear kernel
// writes drive_left/drive_right/steering and the variant's
// `lunco:driveKernel = "external"` sentinel stands ALL of it down — the law
// must cover every port it displaces (see rover_modelica_drive.rhai for the
// skid sibling of this file).
//
// Mounted as `def LunCoProgram "DriveBridge"` on the rover root, so `me` IS
// the rover — sibling lookup by published output name, no name inference.
//
// Removable: if native `inputs:<port>.connect` wiring onto FSW ports lands,
// three connects replace this file.

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
    set(me, "steering", sibling_out(me, "steering"));
}
