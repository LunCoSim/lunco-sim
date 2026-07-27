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

  // Continuous floor force model for better solver stability. rumoca is branch-free
  // (`if` in an equation section reconstructs as literal 0, so f_floor would read zero
  // and the ball would fall straight through), so contact is a continuous gate rather
  // than a test: exactly 0 above the floor, exactly 1 once penetration exceeds h_band.
  // Inside the gate the force is the unchanged compliant spring-damper.
  contact = min(max(-h / h_band, 0.0), 1.0);
  f_floor = (-k_floor * h - d_floor * v) * contact;

  der(v) = -g + f_floor;
end BouncyBall;
