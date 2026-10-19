model MechanicalLossContracts
  "Passive transmission and motor-to-shaft thermal accounting"
  model GearCase
    input Real torque = 1.0;
    input Real speed = 1.0;
    parameter Real efficiency = 0.8;
    parameter Real reduction = 10.0;
    LunCo.Mechanics.Torque source(tau_ref=torque);
    LunCo.Mechanics.GearRatio gear(ratio=reduction, eta=efficiency,
      max_output_torque=20.0);
    LunCo.Mechanics.AvianShaft shaft(speed=speed);
  equation
    connect(source.flange, gear.flange_a);
    connect(gear.flange_b, shaft.flange);
  end GearCase;
  model BearingCase
    parameter Real speed = 2.0;
    parameter Real damping = 0.45;
    LunCo.Mechanics.BearingFriction bearing(d=damping);
    LunCo.Mechanics.AvianShaft shaft(speed=speed);
  equation
    connect(bearing.flange, shaft.flange);
  end BearingCase;
  GearCase drive;
  GearCase backdrive(torque=-1.0);
  GearCase reverse(torque=-1.0, speed=-1.0);
  GearCase reverse_backdrive(speed=-1.0);
  GearCase neutral(torque=0.0);
  GearCase stopped(speed=0.0);
  GearCase ideal(efficiency=1.0);
  GearCase negative_ratio(reduction=-10.0);
  GearCase reversal(speed=2.0*time-3.0);
  BearingCase bearing_forward;
  BearingCase bearing_reverse(speed=-2.0);
  BearingCase bearing_ideal(damping=0.0);
  LunCo.Electrical.IdealVoltageSource rail(voltage=28.0);
  LunCo.Electrical.DCMotor motor(demand=1.0, speed=gear.speed_a);
  LunCo.Mechanics.Torque source(tau_ref=motor.shaft_torque);
  LunCo.Mechanics.GearRatio gear(ratio=10.0, eta=0.8);
  LunCo.Mechanics.BearingFriction bearing(d=0.01);
  LunCo.Mechanics.AvianShaft load(speed=120.0);
  LunCo.Thermal.HeatLoad losses(heat_w=motor.heat+gear.heat+bearing.heat);
  LunCo.Thermal.ThermalMass body(c_th=4000.0, T_init=293.15);
  Real accumulated_heat_j(start=0.0, fixed=true);
  output Real power_balance_w;
  output Real thermal_balance_j;
  output Real thermal_rise_k;
  output Real reversal_heat_w;
equation
  // Sweep all four quadrants, including zero power, without root events.
  connect(rail.p, motor.p);
  connect(source.flange, gear.flange_a);
  connect(gear.flange_b, bearing.flange);
  connect(gear.flange_b, load.flange);
  connect(losses.port, body.port);
  der(accumulated_heat_j) = losses.heat_in_w;
  power_balance_w = motor.electrical_power-load.torque*load.speed-losses.heat_in_w;
  thermal_balance_j = 4000.0*(body.temp_k-293.15)-accumulated_heat_j;
  thermal_rise_k = body.temp_k-293.15;
  reversal_heat_w = reversal.gear.heat;
  annotation(experiment(StopTime=3.0, Interval=0.01, Tolerance=1e-7));
end MechanicalLossContracts;

model GearZeroEfficiency
  extends MechanicalLossContracts.GearCase(gear(eta=0.0));
end GearZeroEfficiency;
model GearExcessEfficiency
  extends MechanicalLossContracts.GearCase(gear(eta=1.1));
end GearExcessEfficiency;
model GearZeroRatio
  extends MechanicalLossContracts.GearCase(gear(ratio=0.0));
end GearZeroRatio;
model GearOverload
  extends MechanicalLossContracts.GearCase(gear(max_output_torque=7.0));
end GearOverload;
model BearingNegativeDamping
  extends MechanicalLossContracts.BearingCase(bearing(d=-0.45));
end BearingNegativeDamping;

model GearDynamicOverload
  LunCo.Mechanics.Torque source;
  LunCo.Mechanics.GearRatio gear(ratio=10.0, eta=0.8, max_output_torque=12.0);
  LunCo.Mechanics.AvianShaft shaft(speed=1.0);
equation
  source.tau_ref = 1.0+time;
  connect(source.flange, gear.flange_a);
  connect(gear.flange_b, shaft.flange);
end GearDynamicOverload;
