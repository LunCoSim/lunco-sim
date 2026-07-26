within LunCo.Mechanics;
// An external torque applied to a flange — the boundary between a causal signal and
// the acausal driveline.
//
// Everything upstream of a driveline is a COMMAND (a throttle, a controller output, a
// solved motor torque); everything inside it is a shared node. This is where the one
// becomes the other, and it is the part `DCMotor` needs in order to put its computed
// torque onto a shaft rather than merely reporting it as an output.
//
// The sign: `tau` is defined as torque INTO the flange, so a source driving the shaft
// forwards pushes torque out of itself. Getting this backwards is the classic way to
// build a driveline that accelerates the wrong way under load, so it is stated once
// here rather than left to each caller.
//
// Note there is no reaction: this applies torque against the world, not against a
// housing. That is correct for a rover, whose motor housings are bolted to a chassis
// Avian already integrates — the reaction is carried by the rigid body, not by this
// equation set. A free-floating machine would need a two-flange source instead.
model Torque
  input Real tau_ref "Commanded torque, N.m";

  Flange flange;
equation
  flange.tau = -tau_ref;
end Torque;
