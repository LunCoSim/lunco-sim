within LunCo.Pointing;
model RateIntegrator "One integrating axis: advance an angle at a commanded rate."
  extends LunCo.Icons.Pointing;
  // The other half of this package's shared mechanism. `ServoAxis` eases an
  // angle ONTO a setpoint; this one has no setpoint at all — it simply turns,
  // which is what an orbiting body's bearing does. One line of ODE, the same
  // line for a sun's azimuth, a scan mirror's sweep, or a spin-stabilised
  // body's phase, so it lives here once.
  //
  // Used as a component:
  //   LunCo.Pointing.RateIntegrator azimuth(y_init = 0.3);
  //   equation azimuth.rate = omega;
  input Real rate "commanded rate (rad/s)";

  parameter Real y_init = 0.0 "angle at t = 0 (rad)";

  output Real y(start = y_init) "integrated angle (rad)";
equation
  der(y) = rate;
end RateIntegrator;
