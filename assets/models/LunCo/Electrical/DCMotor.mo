within LunCo.Electrical;
// The electrical side of a hub motor: a load on the bus. The MECHANICAL side is Avian's
// — the wheel's spin comes out of the physics step and the torque goes back in — so this
// coarse nameplate model takes normalized demand and reports the electrical draw.
// A higher-fidelity variant may replace it with a shaft-coupled machine without
// changing the public `p` connector or demand input.
model DCMotor
  parameter Real efficiency = 0.85 "Electrical-to-mechanical efficiency, 0..1";
  parameter Real rated_power = 2000.0 "Continuous rated shaft power, W";
  // Bus voltage the drive is rated at. Used to turn the nameplate power rating
  // into the rated CURRENT the controller commands — a property of the machine's
  // rating, not a reading of the node.
  parameter Real v_rated = 48.0 "Bus voltage the drive is rated at, V";

  input Real demand "Normalized motor demand, -1..1";

  Pin p;
  Real electrical_power "Electrical power drawn, W";
equation
  // A MOTOR DRIVE IS CURRENT-CONTROLLED. The inner loop of every real controller
  // regulates current (torque ∝ current); the bus voltage sets how much SPEED
  // that current can be pushed to, not how much current is drawn. So demand maps
  // to current, and this is both the physical direction and the causal one.
  p.i = (rated_power / v_rated) * abs(demand) / max(0.01, efficiency);

  // Power drawn FOLLOWS from the current and the voltage actually present.
  electrical_power = p.i * p.v;

  // ⚠ This model HID the loop rather than causing it. As `p.i = power / p.v`,
  // `demand = 0` made the draw zero, `0/p.v` collapsed, and a parked rover's bus
  // solved fine and looked healthy — it would have failed the moment the rover
  // drew current. The solar panel only made it fail at t=0, because its
  // photocurrent is there from the first step. See `SolarPanel` for the full note.
end DCMotor;
