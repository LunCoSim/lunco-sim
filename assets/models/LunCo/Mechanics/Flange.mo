within LunCo.Mechanics;
// The rotational connector — the mechanical peer of `Pin` and `HeatPort`.
//
// `phi` is shared at a node: everything bolted to one shaft turns through the same
// angle. `tau` is a `flow`, so Modelica sums torques at every node to zero — that is
// Newton's third law, and it is why a shaft needs no count of what is mounted on it.
//
// Angle rather than speed is the shared potential, matching Modelica.Mechanics.Rotational:
// differentiate it for speed, but never the reverse, and two components joined here cannot
// disagree about position the way two speed-coupled ones drift apart under integration.
connector Flange
  Real phi "Absolute rotation angle, rad";
  flow Real tau "Torque INTO the flange, N.m";
end Flange;
