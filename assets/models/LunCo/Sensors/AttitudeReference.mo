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

  input Real attitude_quat_w = 1.0 "IMU attitude quaternion W";
  input Real attitude_quat_x = 0.0 "IMU attitude quaternion X";
  input Real attitude_quat_y = 0.0 "IMU attitude quaternion Y";
  input Real attitude_quat_z = 0.0 "IMU attitude quaternion Z";
  input Real desired_tilt_x = 0.0
    "Requested thrust tilt toward navigation +Z (rad)";
  input Real desired_tilt_z = 0.0
    "Requested thrust tilt toward navigation -X (rad)";

  output Real error_x "Body-frame upright error about X";
  output Real error_y "Body-frame upright error about Y";
  output Real error_z "Body-frame upright error about Z";
  output Real estimated_up_x "Estimated world-up direction in body X";
  output Real estimated_up_y "Estimated world-up direction in body Y";
  output Real estimated_up_z "Estimated world-up direction in body Z";

  FrameVectorTransform up_transform(
    quaternion_epsilon = quaternion_epsilon);
  FrameVectorTransform target_transform(
    quaternion_epsilon = quaternion_epsilon);
  Real target_world_x;
  Real target_world_y;
  Real target_world_z;
  Real target_norm;

equation
  // World/navigation +Y expressed in the body frame (the transpose of the
  // body-to-navigation quaternion rotation). The shared transform owns the
  // normalization and transpose used here and by every other consumer.
  up_transform.quaternion_w = attitude_quat_w;
  up_transform.quaternion_x = attitude_quat_x;
  up_transform.quaternion_y = attitude_quat_y;
  up_transform.quaternion_z = attitude_quat_z;
  up_transform.vector_x = 0.0;
  up_transform.vector_y = 1.0;
  up_transform.vector_z = 0.0;
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
  target_transform.quaternion_w = attitude_quat_w;
  target_transform.quaternion_x = attitude_quat_x;
  target_transform.quaternion_y = attitude_quat_y;
  target_transform.quaternion_z = attitude_quat_z;
  target_transform.vector_x = target_world_x / target_norm;
  target_transform.vector_y = target_world_y / target_norm;
  target_transform.vector_z = target_world_z / target_norm;

  // Cross(current body +Y, desired body-frame thrust direction) gives the
  // shortest restoring axis in body coordinates. In components this is
  // (target_body_z, 0, -target_body_x), so zero command reduces to the old
  // upright reference while remaining valid at a 90-degree release attitude.
  error_x = target_transform.body_frame_z;
  error_y = 0.0;
  error_z = -target_transform.body_frame_x;
end AttitudeReference;
