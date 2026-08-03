within LunCo.Sensors;

// Converts one raw Avian ray result into a range measurement.
//
// A miss is invalid.  It is never converted into ideal altitude, a clamped
// maximum, or another value that looks like terrain evidence.
model Altimeter
  extends LunCo.Icons.Sensor;

  parameter Real range_filter_time_constant_s = 0.02
    "Range-rate differentiator time constant (s)";

  input Real ray_distance_m = 0.0 "Raw Avian hit distance (m)";
  input Real ray_hit_valid = 0.0 "Raw Avian hit validity (1 = hit)";
  input Real ray_sample_time = 0.0 "Raw Avian physics sample time (s)";

  output Real range_m "Valid measured ray distance (m)";
  output Real range_rate_mps "Derivative of the valid range (m/s)";
  output Real range_valid "1 when the ray returned a collider";
  output Real range_rate_valid "1 when range rate is meaningful";
  output Real sample_time_s "Physics sample time carried with the reading (s)";

  FilteredDerivative range_filter(
    time_constant_s = range_filter_time_constant_s);

equation
  range_valid = max(0.0, min(1.0, ray_hit_valid));
  range_m = if range_valid > 0.5 then ray_distance_m else 0.0;
  range_filter.u = range_m;
  range_rate_mps = if range_valid > 0.5 then range_filter.y else 0.0;
  range_rate_valid = range_valid;
  sample_time_s = ray_sample_time;
end Altimeter;
