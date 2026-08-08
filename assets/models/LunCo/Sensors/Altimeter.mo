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
  input Real angular_velocity_local_x = 0.0
    "Measured body-frame angular velocity X (rad/s)";
  input Real angular_velocity_local_y = 0.0
    "Measured body-frame angular velocity Y (rad/s)";
  input Real angular_velocity_local_z = 0.0
    "Measured body-frame angular velocity Z (rad/s)";

  output Real range_m "Valid measured ray distance (m)";
  output Real range_rate_mps "Derivative of the valid range (m/s)";
  output Real range_valid "1 when the ray returned a positive range";
  output Real range_confidence
    "Confidence in the range geometry, 0..1";
  output Real range_rate_valid "1 when range rate is meaningful";
  output Real sample_time_s "Physics sample time carried with the reading (s)";
  output Real vertical_projection "Downward projection of the measured ray";
  output Real ray_direction_nav_y "Measured ray direction in navigation Y";

  FilteredDerivative range_filter(
    time_constant_s = range_filter_time_constant_s);

  Real ray_direction_norm;
  Real ray_direction_normalized_x;
  Real ray_direction_normalized_y;
  Real ray_direction_normalized_z;
  Real ray_direction_nav_x;
  Real ray_direction_nav_z;
  Real ray_direction_rate_nav_y;
  Real angular_velocity_world_x;
  Real angular_velocity_world_y;
  Real angular_velocity_world_z;
  Real projected_range_m;
  Real vertical_validity;
  Real range_validity;
  FrameVectorTransform ray_transform;
  FrameVectorTransform angular_velocity_transform;

equation
  // The raw ray is authored in the altimeter frame. Its attitude is supplied
  // by the IMU connection on the vehicle, so a tilted ray is converted into
  // vertical clearance without introducing a world-coordinate dependency.
  ray_direction_norm = sqrt(max(1.0e-12,
    ray_direction_local_x * ray_direction_local_x
      + ray_direction_local_y * ray_direction_local_y
      + ray_direction_local_z * ray_direction_local_z));
  ray_direction_normalized_x = ray_direction_local_x / ray_direction_norm;
  ray_direction_normalized_y = ray_direction_local_y / ray_direction_norm;
  ray_direction_normalized_z = ray_direction_local_z / ray_direction_norm;
  ray_transform.quaternion_w = attitude_quat_w;
  ray_transform.quaternion_x = attitude_quat_x;
  ray_transform.quaternion_y = attitude_quat_y;
  ray_transform.quaternion_z = attitude_quat_z;
  ray_transform.vector_x = ray_direction_normalized_x;
  ray_transform.vector_y = ray_direction_normalized_y;
  ray_transform.vector_z = ray_direction_normalized_z;
  ray_direction_nav_x = ray_transform.world_frame_x;
  ray_direction_nav_y = ray_transform.world_frame_y;
  ray_direction_nav_z = ray_transform.world_frame_z;
  // The range is projected onto navigation vertical.  When the vehicle is
  // rotating, the projection itself changes even if the vehicle has no
  // vertical motion.  Use the measured body rate to remove that geometric
  // term instead of presenting attitude recovery as a climb or descent.
  angular_velocity_transform.quaternion_w = attitude_quat_w;
  angular_velocity_transform.quaternion_x = attitude_quat_x;
  angular_velocity_transform.quaternion_y = attitude_quat_y;
  angular_velocity_transform.quaternion_z = attitude_quat_z;
  angular_velocity_transform.vector_x = angular_velocity_local_x;
  angular_velocity_transform.vector_y = angular_velocity_local_y;
  angular_velocity_transform.vector_z = angular_velocity_local_z;
  angular_velocity_world_x = angular_velocity_transform.world_frame_x;
  angular_velocity_world_y = angular_velocity_transform.world_frame_y;
  angular_velocity_world_z = angular_velocity_transform.world_frame_z;
  ray_direction_rate_nav_y = angular_velocity_world_z * ray_direction_nav_x
    - angular_velocity_world_x * ray_direction_nav_z;
  vertical_projection = max(0.0, -ray_direction_nav_y);
  // Keep the sensor contract continuous for fixed-step participants. A ray
  // validity transition is a measurement confidence change, not a discrete
  // event in the flight software; saturating ramps preserve the same 0/1
  // values for ordinary samples without making the Modelica stepper search for
  // roots at the terrain horizon.
  // Normalize over the remaining projection range, so a level ray reaches
  // full validity even when the minimum projection is close to one.
  vertical_validity = max(0.0, min(1.0,
    (vertical_projection - minimum_vertical_projection)
      / max(1.0 - minimum_vertical_projection, 1.0e-6)));
  range_validity = max(0.0, min(1.0,
    (ray_distance_m - minimum_range_m)
      / max(minimum_range_m, 1.0e-6)));
  // Validity answers whether the instrument has a physical return. Confidence
  // answers whether that return is useful as vertical landing evidence. They
  // are deliberately separate signals: a slanted ray can be real while still
  // being a poor altitude measurement.
  range_valid = max(0.0, min(1.0, ray_hit_valid)) * range_validity;
  range_confidence = range_valid * vertical_validity;
  projected_range_m = max(0.0, ray_distance_m) * vertical_projection;
  range_m = range_valid * projected_range_m;
  // Differentiate the raw slant range, then apply the product rule to the
  // measured vertical projection.  Differentiating the projected range alone
  // makes a rotating sensor report a fictitious vertical speed.
  // Differentiate the raw sensor distance, not the confidence-weighted
  // presentation value.  The latter intentionally ramps from zero when the
  // ray reacquires after an attitude recovery; feeding that ramp into the
  // differentiator would turn confidence growth into a fictitious climb.
  range_filter.u = max(0.0, ray_distance_m);
  range_filter.sample_valid = max(0.0, min(1.0, ray_hit_valid));
  range_rate_mps = range_valid * (range_filter.y * vertical_projection
    - max(0.0, ray_distance_m) * ray_direction_rate_nav_y);
  range_rate_valid = range_confidence;
  sample_time_s = ray_sample_time;
end Altimeter;
