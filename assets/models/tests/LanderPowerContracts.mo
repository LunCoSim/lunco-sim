model LanderPowerContracts
  "Powered, unpowered and recovering flight-command availability"
  Lander powered(control_powered=1.0, guidance_throttle=0.8,
    attitude_hold=1.0, gyro_x=0.1, gyro_y=0.2, gyro_z=-0.1);
  Lander off(control_powered=0.0, guidance_throttle=0.8,
    attitude_hold=1.0, gyro_x=0.1, gyro_y=0.2, gyro_z=-0.1);
  Lander recovering(control_powered=if time < 0.2 then 1.0 else if time < 0.4 then 0.0 else 1.0,
    guidance_throttle=0.8, attitude_hold=1.0, gyro_x=0.1, gyro_y=0.2, gyro_z=-0.1);
end LanderPowerContracts;
