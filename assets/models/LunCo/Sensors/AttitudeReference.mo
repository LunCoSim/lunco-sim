within LunCo.Sensors;

// A body-frame upright reference using the attitude measurement exposed by the IMU.
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

  output Real error_x "Body-frame upright error about X";
  output Real error_y "Body-frame upright error about Y";
  output Real error_z "Body-frame upright error about Z";
  output Real estimated_up_x "Estimated world-up direction in body X";
  output Real estimated_up_y "Estimated world-up direction in body Y";
  output Real estimated_up_z "Estimated world-up direction in body Z";

  FrameVectorTransform up_transform(
    quaternion_epsilon = quaternion_epsilon);

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
  // With body +Y as the engine axis, a positive body-X rotation moves the
  // thrust vector toward +Z.  The restoring error therefore has the same
  // sign as the measured world-up component in body Z: the hold law applies
  // a negative X torque when the vehicle is tilted toward +Z.
  error_x = estimated_up_z;
  error_y = 0.0;
  // A positive body-Z rotation moves body +Y toward -X.  Negating the body-X
  // up component gives the restoring sign for the Z-axis torque.
  error_z = -estimated_up_x;
end AttitudeReference;
