within LunCo.Sensors;

// Converts one raw Avian ray result into a range measurement.
//
// A miss is invalid.  It is never converted into ideal altitude, a clamped
// maximum, or another value that looks like terrain evidence.
model Altimeter
  extends LunCo.Icons.Sensor;

  parameter Real range_filter_time_constant_s = 0.02
    "Range-rate differentiator time constant (s)";
  parameter Real ray_direction_local_x = 0.0
    "Authored ray direction in the sensor frame, X";
  parameter Real ray_direction_local_y = -1.0
    "Authored ray direction in the sensor frame, Y";
  parameter Real ray_direction_local_z = 0.0
    "Authored ray direction in the sensor frame, Z";
  parameter Real minimum_vertical_projection = 0.05
    "Smallest downward ray projection accepted as altitude evidence";
  parameter Real minimum_range_m = 1.0e-3
    "Smallest positive raw range accepted as a hit (m)";

  input Real ray_distance_m = 0.0 "Raw Avian hit distance (m)";
  input Real ray_hit_valid = 0.0 "Raw Avian hit validity (1 = hit)";
  input Real ray_sample_time = 0.0 "Raw Avian physics sample time (s)";
  input Real attitude_quat_w = 1.0 "Measured attitude quaternion W";
  input Real attitude_quat_x = 0.0 "Measured attitude quaternion X";
  input Real attitude_quat_y = 0.0 "Measured attitude quaternion Y";
  input Real attitude_quat_z = 0.0 "Measured attitude quaternion Z";

  output Real range_m "Valid measured ray distance (m)";
  output Real range_rate_mps "Derivative of the valid range (m/s)";
  output Real range_valid "1 when the ray returned a collider";
  output Real range_rate_valid "1 when range rate is meaningful";
  output Real sample_time_s "Physics sample time carried with the reading (s)";
  output Real vertical_projection "Downward projection of the measured ray";
  output Real ray_direction_nav_y "Measured ray direction in navigation Y";

  FilteredDerivative range_filter(
    time_constant_s = range_filter_time_constant_s);

  Real q_norm;
  Real q_w;
  Real q_x;
  Real q_y;
  Real q_z;
  Real ray_direction_norm;
  Real ray_direction_normalized_x;
  Real ray_direction_normalized_y;
  Real ray_direction_normalized_z;
  Real ray_direction_nav_x;
  Real ray_direction_nav_z;
  Real projected_range_m;

equation
  // The raw ray is authored in the altimeter frame. Its attitude is supplied
  // by the IMU connection on the vehicle, so a tilted ray is converted into
  // vertical clearance without introducing a world-coordinate dependency.
  q_norm = sqrt(max(1.0e-12,
    attitude_quat_w * attitude_quat_w
      + attitude_quat_x * attitude_quat_x
      + attitude_quat_y * attitude_quat_y
      + attitude_quat_z * attitude_quat_z));
  q_w = attitude_quat_w / q_norm;
  q_x = attitude_quat_x / q_norm;
  q_y = attitude_quat_y / q_norm;
  q_z = attitude_quat_z / q_norm;
  ray_direction_norm = sqrt(max(1.0e-12,
    ray_direction_local_x * ray_direction_local_x
      + ray_direction_local_y * ray_direction_local_y
      + ray_direction_local_z * ray_direction_local_z));
  ray_direction_normalized_x = ray_direction_local_x / ray_direction_norm;
  ray_direction_normalized_y = ray_direction_local_y / ray_direction_norm;
  ray_direction_normalized_z = ray_direction_local_z / ray_direction_norm;
  ray_direction_nav_x =
    (1.0 - 2.0 * (q_y * q_y + q_z * q_z)) * ray_direction_normalized_x
      + 2.0 * (q_x * q_y + q_w * q_z) * ray_direction_normalized_y
      + 2.0 * (q_x * q_z - q_w * q_y) * ray_direction_normalized_z;
  ray_direction_nav_y =
    2.0 * (q_x * q_y - q_w * q_z) * ray_direction_normalized_x
      + (1.0 - 2.0 * (q_x * q_x + q_z * q_z)) * ray_direction_normalized_y
      + 2.0 * (q_y * q_z + q_w * q_x) * ray_direction_normalized_z;
  ray_direction_nav_z =
    2.0 * (q_x * q_z + q_w * q_y) * ray_direction_normalized_x
      + 2.0 * (q_y * q_z - q_w * q_x) * ray_direction_normalized_y
      + (1.0 - 2.0 * (q_x * q_x + q_y * q_y)) * ray_direction_normalized_z;
  vertical_projection = max(0.0, -ray_direction_nav_y);
  range_valid = max(0.0, min(1.0, ray_hit_valid))
    * (if vertical_projection >= minimum_vertical_projection
        and ray_distance_m >= minimum_range_m then 1.0 else 0.0);
  projected_range_m = max(0.0, ray_distance_m) * vertical_projection;
  range_m = if range_valid > 0.5 then projected_range_m else 0.0;
  range_filter.u = range_m;
  range_rate_mps = if range_valid > 0.5 then range_filter.y else 0.0;
  range_rate_valid = range_valid;
  sample_time_s = ray_sample_time;
end Altimeter;
