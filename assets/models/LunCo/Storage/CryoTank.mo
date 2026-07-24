within LunCo.Storage;
import LunCo.Thermal.HeatPort;

// Cryogenic Propellant Storage Tank: thermal heat ingress drives boil-off rate & pressure.
model CryoTank
  parameter Real m_init_kg = 500.0 "Initial propellant mass, kg";
  parameter Real h_fg = 447000.0 "Latent heat of vaporization, J/kg";
  parameter Real p_max_bar = 25.0 "Maximum relief valve pressure, bar";

  input Real mass_out_flow "Propellant flow rate to thrusters, kg/s";

  Real m_prop_kg(start = m_init_kg) "Remaining propellant mass, kg";
  output Real mass_kg "Current total component mass for CoM & inertia shift, kg";
  output Real boiloff_rate_kgs "Boil-off venting rate, kg/s";
  output Real fill_pct "Tank propellant mass percentage, 0..100 %";

  HeatPort port "Thermal heat ingress port";
equation
  boiloff_rate_kgs = max(0.0, port.Q / max(1000.0, h_fg));
  der(m_prop_kg) = max(-m_prop_kg, -(mass_out_flow + boiloff_rate_kgs));
  mass_kg = m_prop_kg;
  fill_pct = (m_prop_kg / m_init_kg) * 100.0;
end CryoTank;
