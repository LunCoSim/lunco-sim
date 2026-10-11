// tagline: Projectile under gravity with floor collisions
model BouncyBall
  parameter Real g = 9.81 "Gravity";
  parameter Real k_floor = 1000.0 "Floor stiffness";
  parameter Real d_floor = 10.0 "Floor damping";
  parameter Real h_band = 1e-6 "Contact-gate width (m) — see the gate below";

  Real h(start=10.0) "Height";
  Real v(start=0.0) "Velocity";
  Real contact "Contact gate, 0..1 — 1 while the ball penetrates the floor";
  Real f_floor;

equation
  v = der(h);

  // Compliant contact ramps from zero above the floor to one after h_band
  // penetration; the force inside the band uses the spring-damper law.
  contact = min(max(-h / h_band, 0.0), 1.0);
  f_floor = (-k_floor * h - d_floor * v) * contact;

  der(v) = -g + f_floor;
end BouncyBall;
