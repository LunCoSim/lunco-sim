within LunCo.Pointing;
model SunOrbit "Continuous solar orbit dynamics: drives sun azimuth at rate omega."
  parameter Real omega = 0.3 "Sun orbital angular velocity (rad/s)";
  parameter Real sun_azimuth_init = 0.3 "Initial sun azimuth (rad)";

  // BUILT FROM A COMPONENT, not from a bare `der()`, and that is a UI decision
  // as much as a modelling one: the workbench projects a diagram from a class's
  // component instantiations (`import_model_to_diagram_from_ast` returns `None`
  // when there are none), so an equation-only model opens as a blank canvas. It
  // used to be one line — `der(sun_azimuth) = omega` — and the only way to see
  // what it did was to read the source. Now the turning axis is a part, the
  // diagram shows it, and the integration lives in ONE place shared with every
  // other rotating thing in this package (`ServoAxis` is its easing sibling).
  LunCo.Pointing.RateIntegrator azimuth(y_init = sun_azimuth_init);

  output Real sun_azimuth "Sun azimuth output (rad)";
equation
  azimuth.rate = omega;
  sun_azimuth = azimuth.y;
end SunOrbit;
