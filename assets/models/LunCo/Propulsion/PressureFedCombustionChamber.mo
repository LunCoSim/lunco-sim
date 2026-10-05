within LunCo.Propulsion;

model PressureFedCombustionChamber
  "Choked chamber closing the acausal pressure-fed valve network"
  extends CombustionChamber;
equation
  // Both injector outlets share gas-chamber pressure. The passive feed laws
  // and c-star/throat relation solve flow and pressure together.
  fuel_in.pressure_pa = chamber_pressure_pa;
  oxidizer_in.pressure_pa = chamber_pressure_pa;
end PressureFedCombustionChamber;
