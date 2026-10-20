package BatteryEnergyContracts
  "Energy and boundary witnesses for the shared idealized battery"
  model CurrentDemand
    input Real current_a(unit="A") = 1.0;
    LunCo.Electrical.Pin p;
  equation
    p.i = current_a;
  end CurrentDemand;

  model Discharge
    // Generic bench parameters; callers may override them for a source snapshot.
    parameter Real capacity_ah(unit="Ah") = 1.0;
    parameter Real voltage_v(unit="V") = 28.0;
    parameter Real resistance_ohm(unit="Ohm") = 0.0;
    parameter Real initial_soc = 1.0;
    parameter Real reserve_fraction = 0.001;
    parameter Real discharge_hours = 10.0
      "Synthetic current as a fraction of charge capacity";
    LunCo.Electrical.Battery battery(capacity=capacity_ah,
      voltage_nom=voltage_v, R_internal=resistance_ohm, soc_init=initial_soc,
      soc_empty_threshold=reserve_fraction);
    CurrentDemand load(current_a=capacity_ah/discharge_hours);
    output Real delivered_energy_j(unit="J", start=0.0, fixed=true);
    output Real absorbed_energy_j(unit="J", start=0.0, fixed=true);
    output Real heat_energy_j(unit="J", start=0.0, fixed=true);
    output Real stored_energy_j(unit="J");
    output Real series_loss_j(unit="J", start=0.0, fixed=true);
    output Real transferred_charge_ah(unit="Ah", start=0.0, fixed=true);
    output Real nameplate_energy_j(unit="J");
    output Real charge_balance_ah(unit="Ah");
    output Real nominal_depleted_energy_j(unit="J");
    output Real energy_shortfall_j(unit="J");
    output Real state_of_charge;
    output Real terminal_voltage_v(unit="V");
    output Real terminal_current_a(unit="A");
  equation
    connect(battery.p,load.p);
    der(delivered_energy_j) = battery.discharge_power_w;
    der(absorbed_energy_j) = battery.net_power_w;
    der(heat_energy_j) = battery.heat_w;
    stored_energy_j = battery.stored_energy_j;
    der(series_loss_j) = battery.terminal_current_a^2 * resistance_ohm;
    der(transferred_charge_ah) = -battery.terminal_current_a/3600.0;
    nameplate_energy_j = capacity_ah * 3600.0 * voltage_v;
    charge_balance_ah = transferred_charge_ah - capacity_ah*(initial_soc-battery.soc_out);
    nominal_depleted_energy_j = capacity_ah * 3600.0 * voltage_v
      * (initial_soc-battery.soc_out);
    energy_shortfall_j = nominal_depleted_energy_j
      - delivered_energy_j-series_loss_j;
    state_of_charge = battery.soc_out;
    terminal_voltage_v = battery.terminal_voltage_v;
    terminal_current_a = battery.terminal_current_a;
  end Discharge;
  model Cycle
    LunCo.Electrical.Battery battery(capacity=1.0/3600.0,
      voltage_nom=28.0, R_internal=0.1, soc_init=1.0);
    CurrentDemand load(current_a=if time < 1.5 then 1.0
      else if time < 3.5 then -1.0 else 0.0);
    output Real state_of_charge;
    output Real stored_energy_j(unit="J");
    output Real absorbed_energy_j(unit="J", start=0.0, fixed=true);
    output Real heat_energy_j(unit="J", start=0.0, fixed=true);
    output Real terminal_power_w(unit="W");
    output Real heat_w(unit="W");
  equation
    connect(battery.p,load.p);
    state_of_charge = battery.soc_out;
    stored_energy_j = battery.stored_energy_j;
    der(absorbed_energy_j) = battery.net_power_w;
    der(heat_energy_j) = battery.heat_w;
    terminal_power_w = battery.net_power_w;
    heat_w = battery.heat_w;
  end Cycle;
  model PassiveLoad
    parameter Real resistance_ohm = 10.0;
    LunCo.Electrical.Pin p;
  equation
    p.i = p.v / resistance_ohm;
  end PassiveLoad;

  model EmptyLoad
    LunCo.Electrical.Battery battery(soc_init=0.0);
    PassiveLoad load;
    output Real voltage_v = battery.terminal_voltage_v;
    output Real current_a = battery.terminal_current_a;
    output Real discharge_w = battery.discharge_power_w;
    output Real stored_j = battery.stored_energy_j;
  equation
    connect(battery.p,load.p);
  end EmptyLoad;

  model Regeneration
    LunCo.Electrical.Battery battery(capacity=1.0, soc_init=0.5);
    LunCo.Electrical.DCMotor motor(demand=1.0, speed=3600.0);
    output Real state_of_charge = battery.soc_out;
    output Real stored_energy_j = battery.stored_energy_j;
    output Real absorbed_energy_j(unit="J", start=0.0, fixed=true);
    output Real heat_energy_j(unit="J", start=0.0, fixed=true);
    output Real terminal_power_w = battery.net_power_w;
    output Real heat_w = battery.heat_w;
  equation
    connect(battery.p,motor.p);
    der(absorbed_energy_j) = battery.net_power_w;
    der(heat_energy_j) = battery.heat_w;
  end Regeneration;

  model ZeroCapacity
    extends Discharge(capacity_ah=0.0);
  end ZeroCapacity;
  model ZeroVoltage
    extends Discharge(voltage_v=0.0);
  end ZeroVoltage;
  model NegativeResistance
    extends Discharge(resistance_ohm=-0.1);
  end NegativeResistance;
  model NegativeInitialCharge
    extends Discharge(initial_soc=-0.1);
  end NegativeInitialCharge;
  model ExcessInitialCharge
    extends Discharge(initial_soc=1.1);
  end ExcessInitialCharge;
  model ZeroReserve
    extends Discharge(reserve_fraction=0.0);
  end ZeroReserve;
  model FullReserve
    extends Discharge(reserve_fraction=1.0);
  end FullReserve;
end BatteryEnergyContracts;
