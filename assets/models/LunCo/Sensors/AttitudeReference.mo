within LunCo.Sensors;

// A body-frame thrust-vector reference using the attitude measurement exposed by
// the IMU. The desired tilt is expressed in navigation axes; the shared frame
// transform converts that target into the measured body frame before the
// restoring error is formed. This remains well behaved when the vehicle starts
// at a large angle, where adding independent Euler-axis errors is not valid.
//
// Avian publishes the primitive rigid-body quaternion through the IMU sensor
// boundary. Modelica normalizes that measurement and converts navigation +Y
// into the body frame. Accelerometer specific force is deliberately not used as
// a gravity vector here: while the engine is firing, it contains thrust and
// would bias an accelerometer-only attitude estimate toward the nozzle axis.
model AttitudeReference
  extends LunCo.Icons.Sensor;

  parameter Real quaternion_epsilon = 1.0e-12 "Quaternion normalization floor";
  parameter Real heading_projection_epsilon = 1.0e-12
    "Squared horizontal-vector floor; a vertical direction has no azimuth";
  parameter Real heading_forward_body_x = 0.0
    "Body X component of the forward face used for heading";
  parameter Real heading_forward_body_z = 1.0
    "Body Z component of the forward face used for heading";

  input Real attitude_quat_w = 1.0 "IMU attitude quaternion W";
  input Real attitude_quat_x = 0.0 "IMU attitude quaternion X";
  input Real attitude_quat_y = 0.0 "IMU attitude quaternion Y";
  input Real attitude_quat_z = 0.0 "IMU attitude quaternion Z";
  input Real desired_tilt_x = 0.0
    "Requested thrust tilt toward navigation +Z (rad)";
  input Real desired_tilt_z = 0.0
    "Requested thrust tilt toward navigation -X (rad)";
  input Real heading_enable = 0.0
    "Autonomous heading authority; zero preserves free yaw/manual control";
  input Real heading_target_body_x = 0.0 "Target direction measured in body X";
  input Real heading_target_body_y = 0.0 "Target direction measured in body Y";
  input Real heading_target_body_z = 0.0 "Target direction measured in body Z";

  output Real error_x "Body-frame upright error about X";
  output Real error_y "Body-frame autonomous heading error about Y";
  output Real error_z "Body-frame upright error about Z";
  output Real estimated_up_x "Estimated world-up direction in body X";
  output Real estimated_up_y "Estimated world-up direction in body Y";
  output Real estimated_up_z "Estimated world-up direction in body Z";
  output Real heading_error_rad "Signed shortest azimuth error about navigation +Y";
  output Real heading_valid "One when forward and target both have an azimuth";

  FrameVectorTransform up_transform(
    quaternion_epsilon = quaternion_epsilon,
    quaternion_w = attitude_quat_w,
    quaternion_x = attitude_quat_x,
    quaternion_y = attitude_quat_y,
    quaternion_z = attitude_quat_z,
    vector_x = 0.0,
    vector_y = 1.0,
    vector_z = 0.0);
  FrameVectorTransform target_transform(
    quaternion_epsilon = quaternion_epsilon,
    quaternion_w = attitude_quat_w,
    quaternion_x = attitude_quat_x,
    quaternion_y = attitude_quat_y,
    quaternion_z = attitude_quat_z,
    vector_x = target_world_x / target_norm,
    vector_y = target_world_y / target_norm,
    vector_z = target_world_z / target_norm);
  FrameVectorTransform heading_target_transform(
    quaternion_epsilon = quaternion_epsilon,
    quaternion_w = attitude_quat_w, quaternion_x = attitude_quat_x,
    quaternion_y = attitude_quat_y, quaternion_z = attitude_quat_z,
    vector_x = heading_target_body_x, vector_y = heading_target_body_y,
    vector_z = heading_target_body_z);
  FrameVectorTransform heading_forward_transform(
    quaternion_epsilon = quaternion_epsilon,
    quaternion_w = attitude_quat_w, quaternion_x = attitude_quat_x,
    quaternion_y = attitude_quat_y, quaternion_z = attitude_quat_z,
    vector_x = heading_forward_body_x, vector_y = 0.0,
    vector_z = heading_forward_body_z);
  Real target_world_x;
  Real target_world_y;
  Real target_world_z;
  Real target_norm;
  Real heading_cross;
  Real heading_dot;
  Real heading_target_horizontal_sq;
  Real heading_forward_horizontal_sq;

equation
  // World/navigation +Y expressed in the body frame (the transpose of the
  // body-to-navigation quaternion rotation). The shared transform owns the
  // normalization and transpose used here and by every other consumer.
  estimated_up_x = up_transform.body_frame_x;
  estimated_up_y = up_transform.body_frame_y;
  estimated_up_z = up_transform.body_frame_z;
  // Build the desired engine direction in navigation coordinates. Positive
  // X tilt moves body +Y toward +Z; positive Z tilt moves it toward -X. The
  // direction is normalized before conversion so the error remains a pure
  // attitude direction rather than depending on command magnitude.
  target_world_x = -sin(desired_tilt_z);
  target_world_y = 1.0;
  target_world_z = sin(desired_tilt_x);
  target_norm = sqrt(max(quaternion_epsilon,
    target_world_x * target_world_x
      + target_world_y * target_world_y
      + target_world_z * target_world_z));

  // Cross(current body +Y, desired body-frame thrust direction) gives the
  // shortest restoring axis in body coordinates. In components this is
  // (target_body_z, 0, -target_body_x), so zero tilt commands upright while
  // remaining valid at a 90-degree release attitude. Heading adds the Y term.
  error_x = target_transform.body_frame_z;
  // Project both measured directions into the navigation horizontal plane.
  // atan2 retains a restoring command at 180 degrees, where cross alone is
  // zero. No azimuth is requested for a vertical Sun/forward direction.
  heading_target_horizontal_sq = heading_target_transform.world_frame_x^2
    + heading_target_transform.world_frame_z^2;
  heading_forward_horizontal_sq = heading_forward_transform.world_frame_x^2
    + heading_forward_transform.world_frame_z^2;
  heading_valid = noEvent(if heading_target_horizontal_sq > heading_projection_epsilon
    and heading_forward_horizontal_sq > heading_projection_epsilon then 1.0 else 0.0);
  heading_cross = heading_forward_transform.world_frame_z
      * heading_target_transform.world_frame_x
    - heading_forward_transform.world_frame_x * heading_target_transform.world_frame_z;
  heading_dot = heading_forward_transform.world_frame_x
      * heading_target_transform.world_frame_x
    + heading_forward_transform.world_frame_z * heading_target_transform.world_frame_z;
  heading_error_rad = noEvent(if heading_valid > 0.5
    then atan2(heading_cross, heading_dot) else 0.0);
  // Heading yields while thrust points away from its commanded hemisphere.
  // The existing X/Z thrust-vector reference retains recovery priority.
  error_y = max(0.0, min(1.0, heading_enable)) * heading_error_rad
    * max(0.0, min(1.0, target_transform.body_frame_y));
  error_z = -target_transform.body_frame_x;
end AttitudeReference;
