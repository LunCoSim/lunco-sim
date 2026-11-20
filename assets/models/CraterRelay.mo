model CraterRelay "Polar ice mine, water electrolysis, three service vehicles and a hopping lander"
  parameter Real lander_dry_kg = 480;
  parameter Real module_dry_kg = 18 "Baffled tender vessels and regolith bin";
  parameter Real tank_capacity_kg = 12;
  parameter Real hopper_capacity_kg = 96;
  parameter Real ice_fraction = 0.05 "Design assumption for buried ice-rich feed";
  parameter Real hydrogen_fraction = 2.016/18.015 "Hydrogen mass fraction of water";
  parameter Real solar_area_m2 = 6144 "64 sun-facing 12 by 8 metre collectors";
  parameter Real extraction_j_kg = 260000 "Heating, sublimation and recovery per kg of feed";
  parameter Real split_j_kg = 55*3600000*hydrogen_fraction + 1.8e6 "Electrolysis at 55 kWh/kg H2 plus compression per kg of water";
  input Real mine_load;
  input Real mine_unload;
  input Real tails_load;
  input Real tails_dump;
  parameter Real exhaust_velocity_mps = 2150 "Effective vacuum exhaust velocity";
  parameter Real max_thrust_n = 2600;
  input Real altitude;
  input Real vertical_speed;
  input Real gravity;
  input Real contact;
  input Real rover_x;
  input Real rover_z;
  input Real rover_speed;
  input Real depot_x;
  input Real depot_z;
  input Real berth_x;
  input Real berth_z;
  input Real lander_x;
  input Real lander_z;
  input Real speed_x;
  input Real speed_z;
  input Real pad_x;
  input Real pad_z;
  input Real fly "Engine enabled";
  input Real climb "Select hop top instead of final approach";
  input Real load "Depot valves commanded open";
  input Real service "Berth valves commanded open";
  input Real piloted = 0 "Manual command authority from the lander";
  input Real external_throttle = 0;
  input Real pitch = 0;
  input Real roll = 0;
  input Real yaw = 0;
  input Real quat_w = 1;
  input Real quat_x = 0;
  input Real quat_y = 0;
  input Real quat_z = 0;
  input Real angvel_x = 0;
  input Real angvel_y = 0;
  input Real angvel_z = 0;
  output Real fuel_kg(start=8, fixed=true, unit="kg");
  output Real tender_kg(start=0, fixed=true, unit="kg");
  output Real cargo_kg(start=0, fixed=true, unit="kg");
  output Real received_kg(start=0, fixed=true, unit="kg");
  output Real burned_kg(start=0, fixed=true, unit="kg");
  output Real feed_kg(start=50, fixed=true, unit="kg");
  output Real water_kg(start=0, fixed=true, unit="kg");
  output Real h2_kg(start=0.30, fixed=true, unit="kg");
  output Real o2_kg(start=2.38, fixed=true, unit="kg");
  output Real spent_kg(start=30, fixed=true, unit="kg");
  output Real tail_cargo_kg(start=0, fixed=true, unit="kg");
  output Real dumped_kg(start=0, fixed=true, unit="kg");
  output Real split_kg(start=0, fixed=true, unit="kg");
  output Real samples_kg(start=0, fixed=true, unit="kg");
  output Real process_w(unit="W");
  output Real field_w(unit="W");
  output Real battery_soc(unit="%");
  output Real battery_w(unit="W") "Positive when charging";
  output Real battery_kwh(unit="kW.h");
  output Real bus_v(unit="V");
  output Real bus_load_w(unit="W");
  output Real cell_temp_k(start=309,fixed=true,unit="K");
  output Real h2_temp_k(start=285,fixed=true,unit="K");
  output Real o2_temp_k(start=285,fixed=true,unit="K");
  output Real h2_kpa(unit="kPa");
  output Real o2_kpa(unit="kPa");
  output Real mine_module_kg(unit="kg");
  output Real tail_module_kg(unit="kg");
  output Real tail_fill;
  output Real pile_scale;
  output Real tender_h2_kg(unit="kg");
  output Real tender_o2_kg(unit="kg");
  output Real stock_ore_kg(start=12000, fixed=true, unit="kg");
  output Real clearance(unit="m");
  output Real trim_x(unit="N");
  output Real trim_z(unit="N");
  output Real thrust_n(unit="N");
  output Real attitude_torque_x(unit="N.m");
  output Real attitude_torque_y(unit="N.m");
  output Real attitude_torque_z(unit="N.m");
  output Real transfer_kgs(unit="kg/s");
  output Real lander_mass(unit="kg");
  output Real module_mass(unit="kg");
  output Real module_com_z(unit="m");
  output Real module_ix(unit="kg.m2");
  output Real module_iy(unit="kg.m2");
  output Real module_iz(unit="kg.m2");
  output Real fill_scale;
  output Real engine_flow_kgs(unit="kg/s");
  output Real exhaust_mps(unit="m/s");
  output Real rated_thrust_n(unit="N");
  output Real arm_center_m(unit="m");
  output Real umbilical_scale;
  output Real depot_distance(unit="m");
  output Real berth_distance(unit="m");
  output Real pad_error(unit="m");
  LunCo.Electrical.SolarPanel field(area=solar_area_m2,efficiency=0.24,
    peak_power_rating_w=solar_area_m2*1361*0.24,v_mp=800,solar_absorptivity=0.83,
    cell_temperature_k=cell_temp_k);
  LunCo.Electrical.SolarChargeController converter(pv_voltage_v=800,efficiency=0.95,minimum_bus_voltage_v=500);
  LunCo.Electrical.Battery storage(voltage_nom=800,capacity=1250,R_internal=0,soc_init=0.62);
  model RefineryLoad
    LunCo.Electrical.Pin p;
    input Real demand;
  equation
    p.i = demand/max(500,p.v);
  end RefineryLoad;
  RefineryLoad equipment;
protected
  parameter Real hotel_w = 60000 "Controls, heater and pump allowance";
  parameter Real pilot_tilt_rad = 0.22;
  parameter Real pilot_yaw_rate_rad_s = 0.35;
  parameter Real attitude_gain_nm = 740;
  parameter Real rate_gain_nm_s = 760;
  parameter Real rcs_torque_limit_nm = 220;
  parameter Real rcs_arm_m = 1.0;
  LunCo.Sensors.FrameVectorTransform up_in_body(quaternion_w=quat_w,
    quaternion_x=quat_x, quaternion_y=quat_y, quaternion_z=quat_z, vector_y=1.0);
  LunCo.Sensors.FrameVectorTransform rate_in_body(quaternion_w=quat_w,
    quaternion_x=quat_x, quaternion_y=quat_y, quaternion_z=quat_z,
    vector_x=angvel_x, vector_y=angvel_y, vector_z=angvel_z);
  LunCo.Sensors.FrameVectorTransform torque_in_world(quaternion_w=quat_w,
    quaternion_x=quat_x, quaternion_y=quat_y, quaternion_z=quat_z,
    vector_x=body_torque_x, vector_y=body_torque_y, vector_z=body_torque_z);
  Real main_request_n;
  Real rcs_limit;
  Real body_torque_x;
  Real body_torque_y;
  Real body_torque_z;
  Real available_w;
  Real desired_v;
  Real load_gate;
  Real dock_gate;
  Real fuel_load;
  Real ore_load;
  Real ore_drop;
  Real extract;
  Real split;
  Real residue;
  Real reclaim;
  Real tip;
  Real burn;
equation
  depot_distance = sqrt((rover_x-depot_x)^2+(rover_z-depot_z)^2);
  berth_distance = sqrt((rover_x-berth_x)^2+(rover_z-berth_z)^2);
  pad_error = sqrt((lander_x-pad_x)^2+(lander_z-pad_z)^2);
  lander_mass = lander_dry_kg + fuel_kg;
  module_mass = module_dry_kg + tender_kg;
  module_com_z = -0.52*tender_kg/module_mass;
  module_ix = 6 + module_dry_kg*module_com_z^2 + tender_kg*(0.11+(-0.52-module_com_z)^2);
  module_iy = module_ix + 3 + 0.52*tender_kg;
  module_iz = 5 + 0.55*tender_kg;
  mine_module_kg = 32 + cargo_kg;
  tail_module_kg = 32 + tail_cargo_kg;
  desired_v = max(-1.75,min(2.5,0.95*((if climb>0.5 then 7.5 else 0.65)-altitude)));
  main_request_n = if piloted>0.5 then max_thrust_n*max(0,min(1,external_throttle))
    else if fly>0.5 then max(0,lander_mass*(gravity+2.4*(desired_v-vertical_speed))) else 0;
  thrust_n = min(max_thrust_n,min(max(0,fuel_kg)*exhaust_velocity_mps,main_request_n));
  // Upright attitude hold uses bounded reaction-control couples. Pilot pitch and
  // roll select tilt in body axes. Neutral yaw damps rotation without choosing a heading.
  rcs_limit = if contact<0.5 or thrust_n>lander_mass*gravity then
    min(rcs_torque_limit_nm,max(0,fuel_kg)*exhaust_velocity_mps*rcs_arm_m) else 0;
  body_torque_x = max(-rcs_limit,min(rcs_limit,attitude_gain_nm*
    ((if piloted>0.5 then pilot_tilt_rad*pitch else 0)+up_in_body.body_frame_z)
    -rate_gain_nm_s*rate_in_body.body_frame_x));
  body_torque_y = max(-rcs_limit,min(rcs_limit,rate_gain_nm_s*
    ((if piloted>0.5 then pilot_yaw_rate_rad_s*yaw else 0)-rate_in_body.body_frame_y)));
  body_torque_z = max(-rcs_limit,min(rcs_limit,attitude_gain_nm*
    ((if piloted>0.5 then pilot_tilt_rad*roll else 0)-up_in_body.body_frame_x)
    -rate_gain_nm_s*rate_in_body.body_frame_z));
  attitude_torque_x = torque_in_world.world_frame_x;
  attitude_torque_y = torque_in_world.world_frame_y;
  attitude_torque_z = torque_in_world.world_frame_z;
  clearance = sqrt((rover_x-lander_x)^2+(rover_z-lander_z)^2);
  // Opposed vacuum trim nozzles hold the surveyed pad center during free flight.
  trim_x = if piloted<0.5 and thrust_n>1 and (contact<0.5 or climb>0.5) then lander_mass*max(-0.28,min(0.28,0.65*(pad_x-lander_x)-1.6*speed_x)) else 0;
  trim_z = if piloted<0.5 and thrust_n>1 and (contact<0.5 or climb>0.5) then lander_mass*max(-0.28,min(0.28,0.65*(pad_z-lander_z)-1.6*speed_z)) else 0;
  rated_thrust_n = max_thrust_n;
  exhaust_mps = exhaust_velocity_mps;
  engine_flow_kgs = thrust_n/exhaust_velocity_mps;
  burn = engine_flow_kgs+(abs(trim_x)+abs(trim_z)
    +(abs(body_torque_x)+abs(body_torque_y)+abs(body_torque_z))/rcs_arm_m)/exhaust_velocity_mps;
  load_gate = if load>0.5 and depot_distance<1.05 and rover_speed<0.32 then 1 else 0;
  dock_gate = if service>0.5 and fly<0.5 and piloted<0.5 and contact>0.5 and pad_error<0.9
    and berth_distance<1.05 and rover_speed<0.32 then 1 else 0;
  // Stocks are held at separate berths. A tanker can draw only the paired
  // masses actually available in the two product tanks.
  tender_h2_kg = hydrogen_fraction*tender_kg;
  tender_o2_kg = tender_kg-tender_h2_kg;
  fuel_load = load_gate*max(0,min(1.6,min(2*(tank_capacity_kg-tender_kg),
    2*min(h2_kg/hydrogen_fraction,o2_kg/(1-hydrogen_fraction)))));
  transfer_kgs = dock_gate*max(0,min(1.8,min(2*(tank_capacity_kg-fuel_kg),2*tender_kg)));
  ore_load = mine_load*max(0,min(12,min(2*(hopper_capacity_kg-cargo_kg),2*stock_ore_kg)));
  ore_drop = mine_unload*max(0,min(16,min(2*cargo_kg,2*(600-feed_kg))));
  reclaim = tails_load*max(0,min(14,min(2*spent_kg,2*(hopper_capacity_kg-tail_cargo_kg))));
  tip = tails_dump*max(0,min(16,2*tail_cargo_kg));

  // Normal in field-local coordinates for the fixed 38-degree illumination rig.
  field.irradiance=1361;
  field.sun_visibility=1;
  field.target_mount_x=0;
  field.target_mount_y=sin(38*3.14159265/180);
  field.target_mount_z=cos(38*3.14159265/180);
  field.panel_normal_x=0;
  field.panel_normal_y=cos(52*3.14159265/180);
  field.panel_normal_z=sin(52*3.14159265/180);
  der(cell_temp_k)=(field.heat_w/solar_area_m2
    -0.84*5.670374419e-8*(2*cell_temp_k^4-220^4))/8500;
  connect(field.p,converter.pv);
  connect(converter.bus,storage.p);
  connect(storage.p,equipment.p);
  field_w=converter.output_power_w;
  battery_soc=storage.soc_percent;
  battery_kwh=storage.stored_energy_j/3600000;
  battery_w=storage.net_power_w;
  bus_v=storage.terminal_voltage_v;
  available_w=max(0,field_w+300000*min(1,max(0,(battery_soc-12)/3))-hotel_w);
  extract = max(0,min(available_w*0.25/extraction_j_kg,min(2*feed_kg,
    min(2*(420-spent_kg),2*(60-water_kg)/ice_fraction))));
  split = max(0,min((available_w-extract*extraction_j_kg)/split_j_kg,
    min(2*water_kg,min(2*(8-h2_kg)/hydrogen_fraction,2*(64-o2_kg)/(1-hydrogen_fraction)))));
  process_w = extract*extraction_j_kg + split*split_j_kg;
  equipment.demand=process_w+hotel_w;
  bus_load_w=equipment.p.v*equipment.p.i;
  // Each receiver is a rigid cylinder with ellipsoidal heads. Inlet enthalpy,
  // outlet flow work and wall thermal inertia set the bulk gas temperature.
  h2_kpa=h2_kg*4124*h2_temp_k/(1000*3.14159265*1.20^2*(3.6+4*0.60/3));
  o2_kpa=o2_kg*259.84*o2_temp_k/(1000*3.14159265*0.95^2*(3.6+4*0.60/3));
  der(h2_temp_k)=(hydrogen_fraction*(split*(14304*295-10180*h2_temp_k)
    -fuel_load*4124*h2_temp_k)+38*(285-h2_temp_k))/(280000+h2_kg*10180);
  der(o2_temp_k)=((1-hydrogen_fraction)*(split*(918*295-658.16*o2_temp_k)
    -fuel_load*259.84*o2_temp_k)+32*(285-o2_temp_k))/(230000+o2_kg*658.16);
  residue = extract*(1-ice_fraction);
  der(stock_ore_kg) = -ore_load;
  der(cargo_kg) = ore_load-ore_drop;
  der(received_kg) = ore_drop;
  der(feed_kg) = ore_drop-extract;
  der(water_kg) = ice_fraction*extract-split;
  der(h2_kg) = hydrogen_fraction*(split-fuel_load);
  der(o2_kg) = (1-hydrogen_fraction)*(split-fuel_load);
  der(split_kg) = split;
  der(spent_kg) = residue*0.998-reclaim;
  der(samples_kg) = residue*0.002;
  der(tail_cargo_kg) = reclaim-tip;
  der(dumped_kg) = tip;
  der(tender_kg) = fuel_load-transfer_kgs;
  der(fuel_kg) = transfer_kgs-burn;
  der(burned_kg) = burn;
  fill_scale = 0.40*cargo_kg/hopper_capacity_kg;
  tail_fill = 0.40*tail_cargo_kg/hopper_capacity_kg;
  // Self-similar conical heap: fixed footprint-to-height ratio and 1500 kg/m3.
  pile_scale = (max(0,dumped_kg)/(3.14159265*1.7^2*1500/3)+0.000001)^(1.0/3.0);
  umbilical_scale = 0.02+0.98*dock_gate;
  arm_center_m = 0.41*umbilical_scale;
end CraterRelay;
