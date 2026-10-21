package RadiativeExchangeContracts
  model Exchange
    parameter Real initial_temperature_k=300.0;
    parameter Real environment_temperature_k=250.0;
    parameter Real area_m2=2.0;
    parameter Real emissivity=0.87;
    parameter Real capacity_j_per_k=6000.0;
    LunCo.Thermal.ThermalMass mass(c_th=capacity_j_per_k,T_init=initial_temperature_k);
    LunCo.Thermal.Radiator radiation(area=area_m2,emissivity=emissivity,T_sink=environment_temperature_k);
    output Real temperature_k=mass.temp_k;
    output Real rejection_w=radiation.heat_rejection_w;
    output Real balance_w=mass.heat_flow_w+radiation.heat_rejection_w;
    output Real exchange_law_w=radiation.heat_rejection_w
      -emissivity*area_m2*radiation.sigma*(mass.temp_k^4-environment_temperature_k^4);
  equation
    connect(mass.port,radiation.port);
  end Exchange;
  model Cooling
    extends Exchange;
  end Cooling;
  model Heating
    extends Exchange(initial_temperature_k=250.0,environment_temperature_k=350.0);
  end Heating;
  model Equilibrium
    extends Exchange(initial_temperature_k=300.0,environment_temperature_k=300.0);
  end Equilibrium;
  model Mirror
    extends Exchange(emissivity=0.0);
  end Mirror;
  model InvalidEmissivity
    extends Exchange(emissivity=1.01);
  end InvalidEmissivity;
end RadiativeExchangeContracts;
