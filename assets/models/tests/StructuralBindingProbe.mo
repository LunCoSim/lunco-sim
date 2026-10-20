model StructuralBindingProbe
  parameter Integer n = 2 "Number of array elements";
  parameter Real capacity = 8.0 annotation(Evaluate=true);
  parameter Real gain = 2.0;
  Real values[n];
  output Real sample;
equation
  for i in 1:n loop
    values[i] = capacity*gain*i;
  end for;
  sample = sum(values);
end StructuralBindingProbe;
