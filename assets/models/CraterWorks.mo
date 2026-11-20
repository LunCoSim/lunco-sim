model CraterWorks "Solar-powered sample packing demonstrator"
  parameter Real area_m2 = 18.0 "Four photovoltaic wings";
  parameter Real batch_kg = 24.0 "Material in one hopper charge";
  parameter Real packing_energy_j_kg = 3500.0 "Specific work of conveying and packing";
  input Real received_kg "Cumulative regolith delivered at the receiving station";
  input Real target_tilt "Array elevation command, rad";
  input Real line_enabled "Run switch, 0 or 1";
  input Real jog "Momentary manual press control, 0 or 1";
  output Real tilt(start=0, fixed=true, unit="rad");
  output Real solar_w(unit="W");
  output Real line_w(unit="W");
  output Real packed_kg(start=0, fixed=true, unit="kg");
  output Real hopper_kg(unit="kg") "Unpacked regolith remaining in the press hopper";
  output Real ram_angle(unit="rad") "Packing crank deflection";
  output Real shuttle_m(unit="m") "Travel of the two-position packing shuttle";
  LunCo.Electrical.SolarPanel array(area=area_m2, efficiency=0.24);
  LunCo.Electrical.IdealVoltageSource rail(voltage=48);
equation
  der(tilt) = (target_tilt - tilt) / 0.8;
  array.irradiance = 1361;
  array.sun_visibility = 1;
  // Fixed illumination rig at 38 degrees above the bench horizon.
  array.target_mount_x = 0;
  array.target_mount_y = sin(0.6632251158);
  array.target_mount_z = cos(0.6632251158);
  array.panel_normal_x = 0;
  array.panel_normal_y = cos(tilt);
  array.panel_normal_z = sin(tilt);
  connect(array.p, rail.p);
  solar_w = array.power_out;
  hopper_kg = max(0,batch_kg+received_kg-packed_kg);
  line_w = min(1,max(0,max(line_enabled,jog)))*min(solar_w, 2*hopper_kg*packing_energy_j_kg);
  der(packed_kg) = line_w / packing_energy_j_kg;
  shuttle_m = 0.95 * (1 - cos(packed_kg * 1.57079632679490));
  ram_angle = 0.55 * sin(packed_kg * 3.14159265358979);
end CraterWorks;
