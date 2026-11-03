model InitialBoundaryConstraint
  input Real feedback = 0.0;
  Real x(start=0.0, fixed=false);
  output Real sample;
initial equation
  feedback = 0.0;
equation
  der(x) = 0.0;
  sample = x;
end InitialBoundaryConstraint;
