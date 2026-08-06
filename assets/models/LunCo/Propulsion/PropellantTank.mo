within LunCo.Propulsion;

model PropellantTank
  "Lumped liquid-propellant tank with mass and ullage-pressure state"
  extends LunCo.Icons.PropellantTank;

  parameter Real initial_mass_kg = 1000.0;
  parameter Real nominal_pressure_pa = 2.5e6;
  parameter Real pressure_headroom_pa = 5.0e5;
  parameter Real low_fuel_mass_kg = 100.0;
  parameter Real depleted_mass_kg = 0.5;
  parameter Real minimum_mass_kg = 1.0e-6;
  parameter Real event_transition_width_kg = 0.01;
  parameter Real liquid_specific_enthalpy_j_kg = 3.0e5
    "Specific enthalpy of propellant leaving the tank";

  input Real auxiliary_mass_flow_kgs = 0.0
    "Additional demand from a parallel propulsion circuit";
  FluidPort outlet "Propellant outlet to the feed system";
  output Real mass_kg "Remaining liquid propellant, kg";
  output Real pressure_pa "Approximate tank outlet pressure, Pa";
  output Real mass_out_flow_kgs "Actual outlet flow, kg/s";
  output Real low_fuel "Low-fuel event signal";
  output Real depleted "Empty-tank event signal";

  Real mass(start = initial_mass_kg);

equation
  der(mass) = outlet.mass_flow_kgs - max(0.0, auxiliary_mass_flow_kgs);
  mass_kg = max(0.0, mass);
  mass_out_flow_kgs = max(0.0, -outlet.mass_flow_kgs);
  outlet.pressure_pa = nominal_pressure_pa
    + pressure_headroom_pa * max(0.0, min(1.0, mass / max(minimum_mass_kg, initial_mass_kg)));
  outlet.specific_enthalpy_j_kg = liquid_specific_enthalpy_j_kg;
  pressure_pa = nominal_pressure_pa
    + pressure_headroom_pa * max(0.0, min(1.0, mass / max(minimum_mass_kg, initial_mass_kg)));
  low_fuel = max(0.0, min(1.0,
    0.5 + 0.5 * (low_fuel_mass_kg - mass) / max(minimum_mass_kg, event_transition_width_kg)));
  depleted = max(0.0, min(1.0,
    0.5 + 0.5 * (depleted_mass_kg - mass) / max(minimum_mass_kg, event_transition_width_kg)));
end PropellantTank;
