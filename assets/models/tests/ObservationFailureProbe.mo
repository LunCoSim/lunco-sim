model ObservationFailureProbe
  output Real observed;
equation
  observed = sqrt(0.25 - time);
end ObservationFailureProbe;
