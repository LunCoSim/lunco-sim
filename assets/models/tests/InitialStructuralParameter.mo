model InitialStructuralParameter
  parameter Integer n;
  Real x[n](each start=2, each fixed=true);
  output Real sample = n + sum(x);
equation
  der(x) = fill(0, n);
end InitialStructuralParameter;
