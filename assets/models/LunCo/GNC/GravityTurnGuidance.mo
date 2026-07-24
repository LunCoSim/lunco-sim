within LunCo.GNC;

// Gravity Turn Guidance for high-speed orbital / atmospheric entry braking.
// Aligns main engine thrust vector retrograde (opposite to velocity vector) to maximize energy dissipation.
model GravityTurnGuidance
  parameter Real min_speed_m_s = 5.0 "Minimum speed threshold to maintain gravity turn alignment, m/s";

  input Real vel_x "Lander velocity X, m/s";
  input Real vel_y "Lander velocity Y, m/s";
  input Real vel_z "Lander velocity Z, m/s";

  output Real retrograde_x "Normalized retrograde thrust unit vector X";
  output Real retrograde_y "Normalized retrograde thrust unit vector Y";
  output Real retrograde_z "Normalized retrograde thrust unit vector Z";
  output Real speed_m_s "Current lander velocity magnitude, m/s";

  Real vel_norm "Velocity magnitude, m/s";
equation
  vel_norm = sqrt(max(0.01, vel_x^2 + vel_y^2 + vel_z^2));
  speed_m_s = vel_norm;

  // Retrograde unit vector u = -v / ||v||
  retrograde_x = -vel_x / max(min_speed_m_s, vel_norm);
  retrograde_y = -vel_y / max(min_speed_m_s, vel_norm);
  retrograde_z = -vel_z / max(min_speed_m_s, vel_norm);
end GravityTurnGuidance;
