within LunCo.Sensors;

// One quaternion convention for every sensor and guidance frame conversion.
//
// Avian's rigid-body attitude maps body vectors into the navigation/world
// frame.  The transpose maps world vectors back into body coordinates.  Keep
// both directions here so a sensor or controller cannot silently grow its own
// copy of the rotation matrix (or accidentally use the wrong transpose).
model FrameVectorTransform
  parameter Real quaternion_epsilon = 1.0e-12
    "Quaternion normalization floor";

  input Real quaternion_w = 1.0 "Avian attitude quaternion W";
  input Real quaternion_x = 0.0 "Avian attitude quaternion X";
  input Real quaternion_y = 0.0 "Avian attitude quaternion Y";
  input Real quaternion_z = 0.0 "Avian attitude quaternion Z";
  input Real vector_x = 0.0 "Input vector X";
  input Real vector_y = 0.0 "Input vector Y";
  input Real vector_z = 0.0 "Input vector Z";

  output Real world_frame_x "Input treated as body-frame, expressed in world X";
  output Real world_frame_y "Input treated as body-frame, expressed in world Y";
  output Real world_frame_z "Input treated as body-frame, expressed in world Z";
  output Real body_frame_x "Input treated as world-frame, expressed in body X";
  output Real body_frame_y "Input treated as world-frame, expressed in body Y";
  output Real body_frame_z "Input treated as world-frame, expressed in body Z";
  output Real normalized_quaternion_w "Normalized attitude quaternion W";
  output Real normalized_quaternion_x "Normalized attitude quaternion X";
  output Real normalized_quaternion_y "Normalized attitude quaternion Y";
  output Real normalized_quaternion_z "Normalized attitude quaternion Z";
  output Real quaternion_valid "1 when the quaternion is above its floor";

  Real quaternion_norm_sq;
  Real q_w;
  Real q_x;
  Real q_y;
  Real q_z;

equation
  quaternion_norm_sq = max(quaternion_epsilon,
    quaternion_w * quaternion_w + quaternion_x * quaternion_x
      + quaternion_y * quaternion_y + quaternion_z * quaternion_z);
  q_w = quaternion_w / sqrt(quaternion_norm_sq);
  q_x = quaternion_x / sqrt(quaternion_norm_sq);
  q_y = quaternion_y / sqrt(quaternion_norm_sq);
  q_z = quaternion_z / sqrt(quaternion_norm_sq);

  normalized_quaternion_w = q_w;
  normalized_quaternion_x = q_x;
  normalized_quaternion_y = q_y;
  normalized_quaternion_z = q_z;
  quaternion_valid = max(0.0, min(1.0,
    (quaternion_norm_sq - quaternion_epsilon)
      / max(quaternion_epsilon, 1.0e-12)));

  // Body -> navigation/world. This is the Avian attitude convention.
  world_frame_x =
    (1.0 - 2.0 * (q_y * q_y + q_z * q_z)) * vector_x
      + 2.0 * (q_x * q_y - q_w * q_z) * vector_y
      + 2.0 * (q_x * q_z + q_w * q_y) * vector_z;
  world_frame_y =
    2.0 * (q_x * q_y + q_w * q_z) * vector_x
      + (1.0 - 2.0 * (q_x * q_x + q_z * q_z)) * vector_y
      + 2.0 * (q_y * q_z - q_w * q_x) * vector_z;
  world_frame_z =
    2.0 * (q_x * q_z - q_w * q_y) * vector_x
      + 2.0 * (q_y * q_z + q_w * q_x) * vector_y
      + (1.0 - 2.0 * (q_x * q_x + q_y * q_y)) * vector_z;

  // Navigation/world -> body. This is the transpose of the matrix above.
  body_frame_x =
    (1.0 - 2.0 * (q_y * q_y + q_z * q_z)) * vector_x
      + 2.0 * (q_x * q_y + q_w * q_z) * vector_y
      + 2.0 * (q_x * q_z - q_w * q_y) * vector_z;
  body_frame_y =
    2.0 * (q_x * q_y - q_w * q_z) * vector_x
      + (1.0 - 2.0 * (q_x * q_x + q_z * q_z)) * vector_y
      + 2.0 * (q_y * q_z + q_w * q_x) * vector_z;
  body_frame_z =
    2.0 * (q_x * q_z + q_w * q_y) * vector_x
      + 2.0 * (q_y * q_z - q_w * q_x) * vector_y
      + (1.0 - 2.0 * (q_x * q_x + q_y * q_y)) * vector_z;
end FrameVectorTransform;
