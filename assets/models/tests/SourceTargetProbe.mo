connector SourceTargetUnusedConnector
  parameter Real gain = 99;
  input Real unrelated_input = 88;
  output Real unrelated_output;
end SourceTargetUnusedConnector;

model SourceTargetProbe
  parameter Real gain = 3;
  output Real sample(unit = "m") "Selected output";
  model Nested
    input Real nested_input = 77;
    output Real sample(unit = "s") "Nested output";
  end Nested;
equation
  sample = gain;
end SourceTargetProbe;

model SourceTargetUnusedSibling
  parameter Real sibling_parameter = 66;
  input Real sibling_input = 55;
  output Real sibling_output;
equation
  sibling_output = sibling_input;
end SourceTargetUnusedSibling;
