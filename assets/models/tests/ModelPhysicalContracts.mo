model ModelPhysicalContracts
  "Energy and controller-boundary regression using the shared physical models"
  LunCo.Electrical.IdealVoltageSource rail(voltage=28.0);
  LunCo.Electrical.DCMotor forward(demand=1.0, speed=1200.0);
  LunCo.Electrical.DCMotor reverse(demand=-1.0, speed=-1200.0);
  LunCo.Electrical.DCMotor regeneration(demand=1.0, speed=3600.0);
  LunCo.Electrical.DCMotor braking(demand=0.0, speed=1200.0);
  LunCo.Sensors.ElectricalSensor meter;
  LunCo.Thermal.ThermalMass body(c_th=4000.0, T_init=293.15);
  LunCo.Thermal.HeatLoad body_heat(heat_w=100.0);
  LunCo.Sensors.ThermalSensor temperature_probe;
  LunCo.Storage.CryoTank cryogenic(m_init_kg=10.0, h_fg=447000.0,
    saturation_temperature_k=90.0, mass_out_flow=0.01);
  LunCo.Thermal.HeatLoad boiloff_heat(heat_w=447.0);
  LunCo.Electrical.IdealVoltageSource empty_rail(voltage=0.0);
  LunCo.Thermal.ThermostatHeater unpowered_heater;
  LunCo.Thermal.ThermalMass cold_body(T_init=200.0);
  LunCo.Propulsion.PropellantTank fuel(initial_mass_kg=100.0);
  LunCo.Propulsion.PropellantTank oxidizer(initial_mass_kg=100.0);
  LunCo.Propulsion.Turbopump fuel_pump(maximum_flow_kgs=0.3,
    valve_opening=1.0, available_mass_kg=fuel.mass_kg);
  LunCo.Propulsion.Turbopump oxidizer_pump(maximum_flow_kgs=0.75,
    valve_opening=1.0, available_mass_kg=oxidizer.mass_kg);
  LunCo.Propulsion.CombustionChamber chamber;
  Real pumped_mass_kg(start=0.0, fixed=true);
  LunCo.GNC.PIDAxis upper(setpoint=1.0, kp=0.0, ki=1.0,
    integral_limit=0.25, output_limit=100.0);
  LunCo.GNC.PIDAxis lower(setpoint=-1.0, kp=0.0, ki=1.0,
    integral_limit=0.25, output_limit=100.0);
  LunCo.GNC.PIDAxis reversal(setpoint=if time < 2.0 then 1.0 else -1.0,
    kp=0.0, ki=1.0, integral_limit=0.25, output_limit=100.0);
  output Real forward_balance_w;
  output Real reverse_balance_w;
  output Real regeneration_balance_w;
  output Real braking_balance_w;
  output Real forward_torque_nm;
  output Real reverse_torque_nm;
  output Real regeneration_power_w;
  output Real braking_power_w;
  output Real upper_command;
  output Real lower_command;
  output Real reversal_command;
  output Real upper_integral;
  output Real lower_integral;
  output Real reversal_integral;
  output Real meter_voltage_v;
  output Real meter_current_error_a;
  output Real meter_power_w;
  output Real thermal_probe_flow_w;
  output Real body_temperature_k;
  output Real cryogenic_temperature_k;
  output Real boiloff_rate_kgs;
  output Real cryogenic_mass_kg;
  output Real unpowered_heater_heat_w;
  output Real pump_flow_balance_kgs;
  output Real pump_work_balance_w;
  output Real pump_mass_balance_kg;
  output Real pump_chamber_thrust_n;
equation
  connect(rail.p,meter.p);
  connect(meter.n,forward.p);
  connect(rail.p,reverse.p);
  connect(rail.p,regeneration.p);
  connect(rail.p,braking.p);
  connect(body.port,body_heat.port);
  connect(body.port,temperature_probe.port);
  connect(cryogenic.port,boiloff_heat.port);
  connect(empty_rail.p,unpowered_heater.elec_port);
  connect(cold_body.port,unpowered_heater.thermal_port);
  connect(fuel.outlet,fuel_pump.inlet);
  connect(oxidizer.outlet,oxidizer_pump.inlet);
  connect(fuel_pump.outlet,chamber.fuel_in);
  connect(oxidizer_pump.outlet,chamber.oxidizer_in);
  forward_balance_w = forward.electrical_power-forward.heat-forward.mechanical_power_w;
  reverse_balance_w = reverse.electrical_power-reverse.heat-reverse.mechanical_power_w;
  regeneration_balance_w = regeneration.electrical_power-regeneration.heat-regeneration.mechanical_power_w;
  braking_balance_w = braking.electrical_power-braking.heat-braking.mechanical_power_w;
  forward_torque_nm = forward.shaft_torque;
  reverse_torque_nm = reverse.shaft_torque;
  regeneration_power_w = regeneration.electrical_power;
  braking_power_w = braking.electrical_power;
  upper_command = upper.command;
  lower_command = lower.command;
  reversal_command = reversal.command;
  upper_integral = upper.integral;
  lower_integral = lower.integral;
  reversal_integral = reversal.integral;
  meter_voltage_v = meter.v_bus_sensor;
  meter_current_error_a = meter.i_bus_sensor-forward.terminal_current_a;
  meter_power_w = meter.p.v*meter.p.i+meter.n.v*meter.n.i;
  thermal_probe_flow_w = temperature_probe.port.Q;
  body_temperature_k = body.temp_k;
  cryogenic_temperature_k = cryogenic.port.T;
  boiloff_rate_kgs = cryogenic.boiloff_rate_kgs;
  cryogenic_mass_kg = cryogenic.mass_kg;
  unpowered_heater_heat_w = unpowered_heater.heat_out_w;
  pump_flow_balance_kgs = fuel_pump.mass_flow_kgs+oxidizer_pump.mass_flow_kgs-chamber.propellant_flow;
  pump_work_balance_w = fuel_pump.shaft_power_w-fuel_pump.mass_flow_kgs
    *(fuel_pump.outlet.specific_enthalpy_j_kg-fuel.liquid_specific_enthalpy_j_kg);
  der(pumped_mass_kg) = chamber.propellant_flow;
  pump_mass_balance_kg = fuel.mass_kg+oxidizer.mass_kg+pumped_mass_kg-200.0;
  pump_chamber_thrust_n = chamber.thrust_n;
  annotation(experiment(StopTime=3.0,Interval=0.01,Tolerance=1e-7));
end ModelPhysicalContracts;
