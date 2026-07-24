within LunCo.Sensors;

// Landing Leg Touchdown Contact Switch Sensor.
// Detects ground contact force on landing foot, triggering touchdown event & engine shutoff.
model TouchdownSensor
  parameter Real force_threshold_n = 200.0 "Contact force threshold to trigger touchdown switch, N";

  input Real contact_force_n "Measured ground reaction force on landing leg strut, N";
  output Real touchdown "Touchdown contact state (1.0 = landed on ground, 0.0 = airborne)";
equation
  touchdown = max(0.0, min(1.0, (contact_force_n - force_threshold_n) * 0.01));
end TouchdownSensor;
