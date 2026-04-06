model BouncyBall
  parameter Real g = 9.81 "Gravity";
  parameter Real k_floor = 1000.0 "Floor stiffness";
  parameter Real d_floor = 10.0 "Floor damping";
  
  Real h(start=10.0) "Height";
  Real v(start=0.0) "Velocity";
  Real f_floor;

equation
  v = der(h);
  
  // Continuous floor force model for better solver stability
  f_floor = if h < 0 then -k_floor * h - d_floor * v else 0;
  
  der(v) = -g + f_floor;
end BouncyBall;
