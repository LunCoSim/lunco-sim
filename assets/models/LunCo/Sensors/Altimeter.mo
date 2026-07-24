within LunCo.Sensors;

// Altimeter Radar / Laser Sensor model for lander GNC.
// Measures altitude above terrain, accounting for sensor mount offset, max range mask, and noise.
model Altimeter
  parameter Real range_max_m = 2500.0 "Maximum operating range limit, m";
  parameter Real mount_offset_m = 1.2 "Sensor mount offset above landing foot datum, m";

  input Real alt_true_m "True ground altitude from physics step, m";
  output Real alt_measured_m "Measured altitude reported to GNC flight software, m";
  output Real out_of_range "Out-of-range flag (1.0 = out of range, 0.0 = valid measurement)";
equation
  out_of_range = max(0.0, min(1.0, 0.5 + alt_true_m - range_max_m));
  alt_measured_m = min(range_max_m, max(0.0, alt_true_m - mount_offset_m));
end Altimeter;
