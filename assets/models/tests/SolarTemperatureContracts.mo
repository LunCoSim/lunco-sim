package SolarTemperatureContracts
  "Synthetic temperature and cooling witnesses, not flight-panel calibration"
  model VoltageDrive
    parameter Real voltage_v = 48.0;
    LunCo.Electrical.Pin p;
  equation
    p.v = voltage_v;
  end VoltageDrive;
  model StaticPanel
    parameter Real temperature_k = 298.15;
    parameter Real irradiance_w_m2 = 1000.0;
    parameter Real voltage_v = 48.0;
    LunCo.Electrical.SolarPanel panel(area=0.7, efficiency=0.35,
      peak_power_rating_w=270.0, cell_temperature_k=temperature_k,
      irradiance=irradiance_w_m2, sun_visibility=1.0,
      target_mount_x=0.0, target_mount_y=0.0, target_mount_z=1.0,
      panel_normal_x=0.0, panel_normal_y=0.0, panel_normal_z=1.0);
    VoltageDrive rail(voltage_v=voltage_v);
    output Real available_w = panel.available_power_w;
    output Real delivered_w = panel.power_out;
    output Real heat_w = panel.heat_w;
    output Real factor = panel.temperature_factor;
    output Real energy_balance_w = panel.incident_power_w-panel.reflected_power_w
      -panel.power_out-panel.heat_w;
  equation
    connect(panel.p,rail.p);
  end StaticPanel;
  model Reference
    extends StaticPanel;
  end Reference;
  model Hot
    extends StaticPanel(temperature_k=348.15);
  end Hot;
  model Cold
    extends StaticPanel(temperature_k=248.15);
  end Cold;
  model HotNameplate
    extends StaticPanel(temperature_k=348.15,irradiance_w_m2=2000.0);
  end HotNameplate;
  model ColdNameplate
    extends StaticPanel(temperature_k=248.15,irradiance_w_m2=2000.0);
  end ColdNameplate;
  model HighBus
    extends StaticPanel(voltage_v=96.0);
  end HighBus;
  model ExtremeHot
    extends StaticPanel(temperature_k=600.0);
  end ExtremeHot;
  model InvalidTemperature
    extends StaticPanel(temperature_k=0.0);
  end InvalidTemperature;
  model CoupledPanel
    parameter Real cooling_capacity_w = 100.0;
    parameter Real initial_temperature_k = 348.15;
    parameter Real irradiance_w_m2 = 1000.0;
    parameter Boolean disconnect_heat = false;
    LunCo.Electrical.SolarPanel panel(area=0.7,efficiency=0.35,
      peak_power_rating_w=270.0,cell_temperature_k=mass.temp_k,
      irradiance=irradiance_w_m2,sun_visibility=1.0,
      target_mount_x=0.0,target_mount_y=0.0,target_mount_z=1.0,
      panel_normal_x=0.0,panel_normal_y=0.0,panel_normal_z=1.0);
    VoltageDrive rail;
    LunCo.Thermal.ThermalMass mass(c_th=1000.0,T_init=initial_temperature_k);
    LunCo.Thermal.HeatLoad heat(heat_w=if disconnect_heat then 0.0 else panel.heat_w);
    LunCo.Thermal.LimitedCoolingSink cooler(maximum_heat_rejection_w=cooling_capacity_w,
      minimum_temperature_k=298.15,conductance_w_per_k=20.0);
    output Real temperature_k = mass.temp_k;
    output Real available_w = panel.available_power_w;
    output Real heat_w = panel.heat_w;
    output Real cooling_w = cooler.heat_rejection_w;
    output Real generated_heat_j(start=0.0,fixed=true);
    output Real rejected_heat_j(start=0.0,fixed=true);
    output Real thermal_balance_j;
  equation
    connect(panel.p,rail.p);
    connect(heat.port,mass.port);
    connect(cooler.port,mass.port);
    der(generated_heat_j)=panel.heat_w;
    der(rejected_heat_j)=cooler.heat_rejection_w;
    thermal_balance_j=1000.0*(mass.temp_k-initial_temperature_k)
      -generated_heat_j+rejected_heat_j;
  end CoupledPanel;
  model StrongCooling
    extends CoupledPanel(cooling_capacity_w=600.0);
  end StrongCooling;
  model WeakCooling
    extends CoupledPanel;
  end WeakCooling;
  model Cooldown
    extends CoupledPanel(initial_temperature_k=318.15,irradiance_w_m2=0.0);
  end Cooldown;
  model BelowSink
    extends CoupledPanel(initial_temperature_k=288.15,irradiance_w_m2=0.0);
  end BelowSink;
  model DisconnectedHeat
    extends CoupledPanel(disconnect_heat=true);
  end DisconnectedHeat;
  model InvalidCooling
    extends CoupledPanel(cooling_capacity_w=-1.0);
  end InvalidCooling;
end SolarTemperatureContracts;
