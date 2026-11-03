model InitialInheritedParameter
  extends InitialInheritedParameterBase;
end InitialInheritedParameter;

model InitialInheritedParameterBase
  parameter Real k = 6.0;
  output Real sample;
equation
  sample = k;
end InitialInheritedParameterBase;
