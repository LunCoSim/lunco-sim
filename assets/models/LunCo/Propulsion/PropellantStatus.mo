within LunCo.Propulsion;

model PropellantStatus
  "Vehicle-level propellant telemetry and event aggregation"
  extends LunCo.Icons.PropellantStatus;

  parameter Real low_fuel_mass_kg = 200.0;
  parameter Real depleted_mass_kg = 1.0;
  parameter Real initial_propellant_mass_kg = 2000.0;
  parameter Real minimum_mass_kg = 1.0e-6;
  parameter Real event_transition_width_kg = 0.01;

  input Real fuel_mass_kg = 0.0;
  input Real oxidizer_mass_kg = 0.0;
  output Real propellant_mass_kg;
  output Real propellant_used_kg;
  output Real low_fuel;
  output Real depleted;

equation
  propellant_mass_kg = max(0.0, fuel_mass_kg) + max(0.0, oxidizer_mass_kg);
  propellant_used_kg = max(0.0, initial_propellant_mass_kg - propellant_mass_kg);
  low_fuel = max(0.0, min(1.0,
    0.5 + 0.5 * (low_fuel_mass_kg - propellant_mass_kg)
      / max(minimum_mass_kg, event_transition_width_kg)));
  depleted = max(0.0, min(1.0,
    0.5 + 0.5 * (depleted_mass_kg - propellant_mass_kg)
      / max(minimum_mass_kg, event_transition_width_kg)));
end PropellantStatus;
