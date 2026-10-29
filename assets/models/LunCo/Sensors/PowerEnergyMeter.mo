within LunCo.Sensors;

model PowerEnergyMeter
  "Observe signed power and accumulate energy without loading its source"
  extends LunCo.Icons.Sensor;

  input Real power_w(unit="W") "Authoritative signed power observation";
  input Real enabled = 1.0 "Count while greater than one half";
  output Real positive_energy_wh(unit="W.h", start=0, fixed=true);
  output Real negative_energy_wh(unit="W.h", start=0, fixed=true);
  output Real net_energy_wh(unit="W.h");
  output Real elapsed_s(unit="s", start=0, fixed=true);
equation
  der(positive_energy_wh) = if enabled > 0.5 then max(power_w, 0.0)/3600.0 else 0.0;
  der(negative_energy_wh) = if enabled > 0.5 then max(-power_w, 0.0)/3600.0 else 0.0;
  net_energy_wh = positive_energy_wh - negative_energy_wh;
  der(elapsed_s) = if enabled > 0.5 then 1.0 else 0.0;
end PowerEnergyMeter;
