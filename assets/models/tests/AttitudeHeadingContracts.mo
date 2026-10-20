package AttitudeHeadingContracts
  "Analytic witnesses for thrust-priority heading and its authority gate"
  model Aligned
    parameter Real qw = 1.0;
    parameter Real qx = 0.0;
    parameter Real qy = 0.0;
    parameter Real qz = 0.0;
    parameter Real sx = 0.0;
    parameter Real sy = 0.0;
    parameter Real sz = 1.0;
    parameter Real enabled = 1.0;
    parameter Real rate = 0.0;
    LunCo.Sensors.AttitudeReference reference(
      attitude_quat_w=qw, attitude_quat_x=qx,
      attitude_quat_y=qy, attitude_quat_z=qz,
      heading_target_body_x=sx, heading_target_body_y=sy,
      heading_target_body_z=sz, heading_enable=enabled,
      heading_rate_body_y=rate);
    output Real heading = reference.heading_error_rad;
    output Real valid = reference.heading_valid;
    output Real ex = reference.error_x;
    output Real ey = reference.error_y;
    output Real ez = reference.error_z;
    output Real ready = reference.heading_ready;
  end Aligned;
  model Right
    extends Aligned(sx=1.0, sz=0.0);
  end Right;
  model Left
    extends Aligned(sx=-1.0, sz=0.0);
  end Left;
  model Behind
    extends Aligned(sz=-1.0);
  end Behind;
  model Disabled
    extends Right(enabled=0.0);
  end Disabled;
  model Overhead
    extends Aligned(sy=1.0, sz=0.0);
  end Overhead;
  model Tilted
    // +30 degrees about X. World +Z expressed in body coordinates.
    extends Aligned(qw=0.9659258262890683,qx=0.25881904510252074,
      sy=0.5,sz=0.8660254037844386);
  end Tilted;
  model Yawed
    // +90 degrees about Y, target world +Z: turn back by -90 degrees.
    extends Aligned(qw=0.7071067811865476,qy=0.7071067811865476,
      sx=-1.0,sz=0.0);
  end Yawed;
  model Inverted
    extends Right(qw=0.0,qx=1.0);
  end Inverted;
  model FastAligned
    extends Aligned(rate=0.1);
  end FastAligned;
  model DescentInhibited
    parameter Real permission = 0.0;
    LunCo.GNC.PositionPID3D guidance(initial_pos_y=60.0,
      target_y=0.0,descent_enable=permission,vehicle_mass=1000.0,mass_properties_valid=1.0,
      imu_coordinate_accel_local_y=1.62);
    output Real position_command = guidance.pid_y.setpoint;
    output Real rate_command = guidance.descent_rate_command;
  end DescentInhibited;
  model DescentEnabled
    extends DescentInhibited(permission=1.0);
  end DescentEnabled;
end AttitudeHeadingContracts;
