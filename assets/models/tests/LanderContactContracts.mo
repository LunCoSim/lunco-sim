package LanderContactContracts
  "Independent pad cutoff, settled handoff, and retained damping witnesses"
  model SoftContact
    parameter Real contact = 1.0;
    parameter Real other_contacts = 0.0;
    parameter Real rate = 0.02;
    parameter Real vertical_speed = -0.1;
    parameter Real ground_speed = 0.0;
    parameter Real upright = 1.0;
    parameter Real cutoff_accepted = 0.0;
    parameter Real handoff_accepted = 0.0;
    parameter Real strut_speed = 0.0;
    parameter Real cutoff_rate_limit = 0.05;
    Lander controller(leg_contact_px=contact,
      leg_contact_nx=other_contacts,leg_contact_pz=other_contacts,
      leg_contact_nz=other_contacts,gyro_x=rate,
      navigation_velocity_y=vertical_speed,navigation_velocity_x=ground_speed,
      upright_axis_y=upright,leg_velocity_px=strut_speed,
      engine_cutoff_angular_speed_rad_s=cutoff_rate_limit,
      attitude_hold=1.0,attitude_error_x=0.2,guidance_throttle=0.6,
      landing_engine_cutoff=cutoff_accepted,landing_handoff=handoff_accepted);
    output Real cutoff = controller.engine_cutoff_contact;
    output Real settled = controller.landing_contact;
    output Real throttle = controller.throttle;
    output Real torque = controller.torque_x;
  end SoftContact;
  model Airborne
    extends SoftContact(contact=0.0);
  end Airborne;
  model FastSpin
    extends SoftContact(rate=0.1);
  end FastSpin;
  model CutoffRateBoundary
    extends SoftContact(rate=0.05);
  end CutoffRateBoundary;
  model StricterCutoffRate
    extends SoftContact(cutoff_rate_limit=0.01);
  end StricterCutoffRate;
  model FastGround
    extends SoftContact(ground_speed=0.2);
  end FastGround;
  model FastDescent
    extends SoftContact(vertical_speed=-0.3);
  end FastDescent;
  model Tipped
    extends SoftContact(upright=0.8);
  end Tipped;
  model QuietFourPads
    extends SoftContact(other_contacts=1.0,rate=0.0,vertical_speed=0.0);
  end QuietFourPads;
  model MovingStrut
    extends QuietFourPads(strut_speed=0.5);
  end MovingStrut;
  model SettledRateBoundary
    extends QuietFourPads(rate=0.005);
  end SettledRateBoundary;
  model AcceptedCutoff
    extends SoftContact(cutoff_accepted=1.0);
  end AcceptedCutoff;
  model AcceptedHandoff
    extends SoftContact(handoff_accepted=1.0);
  end AcceptedHandoff;
end LanderContactContracts;
