model WrenchAllocatorContract
  "Analytical bounded-allocation cases, independent of vehicle geometry"
  LunCo.Actuation.WrenchAllocator orthogonal(
    actuator_count=6, allocation_iterations=64,
    wrench_matrix=[1,0,0,0,0,0; 0,1,0,0,0,0; 0,0,1,0,0,0;
      0,0,0,1,0,0; 0,0,0,0,1,0; 0,0,0,0,0,1],
    lower_command=fill(-1.0,6), upper_command=fill(1.0,6));
  LunCo.Actuation.WrenchAllocator coupled(
    actuator_count=3, allocation_iterations=64,
    wrench_matrix=[1,1,0; 0,1,1; 0,0,0; 0,0,0; 0,0,0; 0,0,0],
    lower_command=fill(0.0,3), upper_command=fill(1.0,3));
  LunCo.Actuation.WrenchAllocator powerless(
    actuator_count=1, allocation_iterations=64,
    wrench_matrix=[0; 0; 0; 0; 0; 0], lower_command={0.25}, upper_command={1.0});
equation
  orthogonal.desired_force_x=0.25;
  orthogonal.desired_force_y=-0.4;
  orthogonal.desired_force_z=2.0;
  orthogonal.desired_torque_x=-3.0;
  orthogonal.desired_torque_y=0.0;
  orthogonal.desired_torque_z=0.7;
  coupled.desired_force_x=1.0;
  coupled.desired_force_y=1.0;
  coupled.desired_force_z=0.0;
  coupled.desired_torque_x=0.0;
  coupled.desired_torque_y=0.0;
  coupled.desired_torque_z=0.0;
  powerless.desired_force_x=1.0;
  powerless.desired_force_y=0.0;
  powerless.desired_force_z=0.0;
  powerless.desired_torque_x=0.0;
  powerless.desired_torque_y=0.0;
  powerless.desired_torque_z=0.0;
end WrenchAllocatorContract;
