within LunCo.Storage;

// Prescribed-temperature phase-change study: absorbed heat drives boil-off.
// This component does not predict saturation temperature or tank pressure.
model CryoTank
  extends LunCo.Icons.Storage;
  parameter Real m_init_kg = 500.0 "Initial propellant mass, kg";
  parameter Real h_fg = 447000.0 "Latent heat of vaporization, J/kg";
  parameter Real saturation_temperature_k(unit="K")
    "Required phase-change temperature from the authored propellant operating point";

  input Real mass_out_flow "Propellant flow rate to thrusters, kg/s";

  Real m_prop_kg(start = m_init_kg) "Remaining propellant mass, kg";
  output Real mass_kg "Current total component mass for CoM & inertia shift, kg";
  output Real boiloff_rate_kgs "Boil-off venting rate, kg/s";
  output Real fill_pct "Tank propellant mass percentage, 0..100 %";
  output Real heat_ingress_w(unit="W") "Thermal power entering the cryogenic tank";

  LunCo.Thermal.HeatPort port "Thermal heat ingress port";
equation
  assert(saturation_temperature_k > 0.0,
    "CryoTank requires a positive authored saturation temperature");
  port.T = saturation_temperature_k;
  boiloff_rate_kgs = max(0.0, port.Q / max(1000.0, h_fg));
  der(m_prop_kg) = max(-m_prop_kg, -(mass_out_flow + boiloff_rate_kgs));
  mass_kg = m_prop_kg;
  fill_pct = (m_prop_kg / m_init_kg) * 100.0;
  heat_ingress_w = port.Q;
end CryoTank;
