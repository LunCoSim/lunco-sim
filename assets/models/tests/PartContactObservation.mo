model PartContactObservation
  input Real lower_force(unit = "N");
  input Real upper_force(unit = "N");
  output Real supported_force(unit = "N");
equation
  supported_force = lower_force - upper_force;
end PartContactObservation;
