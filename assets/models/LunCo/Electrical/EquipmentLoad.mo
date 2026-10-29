within LunCo.Electrical;
model EquipmentLoad
  "Aggregated regulated avionics/instrument load, with explicit assumed input power"
  extends LunCo.Electrical.RegulatedLoad;
  extends LunCo.Icons.ElectricalControl;
  parameter Real operating_power_w(unit="W") = 50.0
    "Illustrative aggregate; replace with measured equipment duty cycles";
  output Real heat_w(unit="W");
equation
  requested_power_w = operating_power_w;
  heat_w = power_draw_w;
end EquipmentLoad;
