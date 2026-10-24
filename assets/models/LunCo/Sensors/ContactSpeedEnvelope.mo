within LunCo.Sensors;

model ContactSpeedEnvelope
  "Validity of a contact model at an explicitly qualified normal approach speed"
  extends LunCo.Icons.Sensor;
  input Real contact_speed_mps(unit="m/s")
    "Maximum native pre-solver closing speed of the observed contact shapes";
  input Real maximum_speed_mps(unit="m/s")
    "Authored positive validity limit; this is not a material damage rating";
  output Real exceedance_ratio(unit="1")
    "Zero within the envelope; measured speed divided by the limit outside it";
  output Real speed_limit_mps(unit="m/s")
    "The admitted limit for an event consumer's retained explanation";
equation
  assert(maximum_speed_mps > 0.0,
    "ContactSpeedEnvelope requires an explicit positive validity limit");
  assert(contact_speed_mps >= 0.0,
    "ContactSpeedEnvelope requires a nonnegative closing-speed observation");
  speed_limit_mps = maximum_speed_mps;
  exceedance_ratio = noEvent(if contact_speed_mps > maximum_speed_mps
    then contact_speed_mps / maximum_speed_mps else 0.0);
end ContactSpeedEnvelope;
