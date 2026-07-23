within;
model SunOrbit "Continuous solar orbit dynamics: drives sun azimuth at rate omega."
  parameter Real omega = 0.3 "Sun orbital angular velocity (rad/s)";
  parameter Real sun_azimuth_init = 0.3 "Initial sun azimuth (rad)";

  output Real sun_azimuth(start = sun_azimuth_init) "Sun azimuth output (rad)";
equation
  der(sun_azimuth) = omega;
end SunOrbit;
