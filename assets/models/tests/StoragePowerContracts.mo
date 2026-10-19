model StoragePowerContracts
  "Storage conservation and closed electrical/RF/thermal brownout contract"
  model VoltageDrive
    input Real voltage;
    LunCo.Electrical.Pin p;
  equation
    p.v=voltage;
  end VoltageDrive;
  VoltageDrive rail(voltage=if time<0.5 then 0.0 elseif time<1.5 then 28.0
    elseif time<2.0 then 19.0 elseif time<3.0 then 0.0 elseif time<3.5 then 19.0
    elseif time<4.0 then 28.0 elseif time<5.0 then 0.5 elseif time<6.0 then -5.0 else 28.0);
  LunCo.Comms.DataBuffer buffer(capacity_mb=1.0,
    rate_in_kbps=if time<2.0 then 8000.0 elseif time<4.0 then 0.0
      elseif time<5.5 then 4000.0 elseif time<6.0 then 8000.0 else 0.0,
    rate_out_kbps=if time<2.0 then 0.0 elseif time<4.0 then 16000.0
      elseif time<6.0 then 4000.0 else 0.0);
  LunCo.Comms.DataBuffer full_buffer(capacity_mb=1.0,buffer_init_mb=1.0,
    rate_in_kbps=4000.0,rate_out_kbps=4000.0);
  LunCo.Storage.MassMemory memory(capacity_gb=1.0,
    write_rate_gbps=if time<2.0 then 8.0 else 0.0,
    read_rate_gbps=if time<2.0 then 0.0 else 8.0);
  LunCo.Comms.Transmitter radio(tx_active=link.tx_active);
  LunCo.Comms.Transmitter bench_radio(tx_active=0.5);
  LunCo.Electrical.CameraPayload camera(active=if time<6.0 then 1.0 else 0.0);
  LunCo.Electrical.OnboardComputer computer(gnc_active=1.0);
  LunCo.Electrical.HeadlightController lamp(enable=1.0);
  LunCo.Comms.CommsLink link(tx_power_w=radio.available_rf_power_w,
    data_in_bps=camera.data_rate_out_bps, link_range_m=100.0,link_connected=1.0,
    buffer_capacity_bits=8.0e6,T_filter=0.02,rate_efficiency=0.1);
  LunCo.Thermal.ThermalMass body(c_th=1000.0,T_init=250.0);
  LunCo.Thermal.HeatLoad equipment_heat(heat_w=radio.heat_w+bench_radio.heat_w+camera.heat_w+memory.heat_w+computer.heat_w);
  Real buffer_offered_mb(start=0.0,fixed=true);
  Real buffer_delivered_mb(start=0.0,fixed=true);
  Real buffer_dropped_mb(start=0.0,fixed=true);
  Real memory_offered_gb(start=0.0,fixed=true);
  Real memory_delivered_gb(start=0.0,fixed=true);
  Real memory_rejected_gb(start=0.0,fixed=true);
  Real link_offered_bits(start=0.0,fixed=true);
  Real link_delivered_bits(start=0.0,fixed=true);
  Real heat_j(start=0.0,fixed=true);
  output Real buffer_balance_mb;
  output Real memory_balance_gb;
  output Real link_balance_bits;
  output Real source_balance_w;
  output Real radio_balance_w;
  output Real camera_balance_w;
  output Real bench_radio_balance_w;
  output Real memory_power_balance_w;
  output Real computer_balance_w;
  output Real lamp_balance_w;
  output Real thermal_balance_j;
  output Real pin_voltage_v;
  output Real radio_heat_w;
  output Real body_temperature_k;
equation
  connect(rail.p,memory.p);
  connect(rail.p,radio.p);
  connect(rail.p,bench_radio.p);
  connect(rail.p,camera.p);
  connect(rail.p,computer.p);
  connect(rail.p,lamp.p);
  connect(body.port,equipment_heat.port);
  der(buffer_offered_mb)=buffer.rate_in_kbps/8000.0;
  der(buffer_delivered_mb)=buffer.delivered_rate_kbps/8000.0;
  der(buffer_dropped_mb)=buffer.dropped_rate_kbps/8000.0;
  der(memory_offered_gb)=memory.write_rate_gbps/8.0;
  der(memory_delivered_gb)=memory.delivered_read_rate_gbps/8.0;
  der(memory_rejected_gb)=memory.rejected_write_rate_gbps/8.0;
  der(link_offered_bits)=camera.data_rate_out_bps;
  der(link_delivered_bits)=link.delivered_rate_bps;
  der(heat_j)=equipment_heat.heat_w;
  buffer_balance_mb=buffer.stored_data_mb+buffer_delivered_mb+buffer_dropped_mb-buffer_offered_mb;
  memory_balance_gb=memory.stored_data_gb+memory_delivered_gb+memory_rejected_gb-memory_offered_gb;
  link_balance_bits=link.buffer_bits+link_delivered_bits+link.lost_bits-link_offered_bits;
  source_balance_w=-rail.p.v*rail.p.i-memory.power_draw_w-radio.power_draw_w
    -camera.power_draw_w-computer.power_draw_w-lamp.power_draw_w-bench_radio.power_draw_w;
  radio_balance_w=radio.p.v*radio.p.i-radio.rf_power_out_w-radio.heat_w;
  bench_radio_balance_w=bench_radio.p.v*bench_radio.p.i-bench_radio.rf_power_out_w-bench_radio.heat_w;
  camera_balance_w=camera.p.v*camera.p.i-camera.heat_w;
  memory_power_balance_w=memory.p.v*memory.p.i-memory.heat_w;
  computer_balance_w=computer.p.v*computer.p.i-computer.heat_w;
  lamp_balance_w=lamp.p.v*lamp.p.i-lamp.power_draw_w;
  thermal_balance_j=1000.0*(body.temp_k-250.0)-heat_j;
  pin_voltage_v=rail.p.v;
  radio_heat_w=radio.heat_w;
  body_temperature_k=body.temp_k;
end StoragePowerContracts;

model StoreZeroCapacity
  extends StoragePowerContracts(buffer(capacity_mb=0.0));
end StoreZeroCapacity;
model StoreInvalidInitialFill
  extends StoragePowerContracts(buffer(buffer_init_mb=2.0));
end StoreInvalidInitialFill;
model StoreNegativeRequest
  extends StoragePowerContracts(buffer(rate_in_kbps=-1.0));
end StoreNegativeRequest;
model StoreDynamicNegativeRequest
  extends StoragePowerContracts(buffer(rate_in_kbps=if time<0.2 then 0.0 else -1.0));
end StoreDynamicNegativeRequest;
model MemoryNegativePower
  extends StoragePowerContracts(memory(p_standby_w=-1.0));
end MemoryNegativePower;
model RadioInvalidEfficiency
  extends StoragePowerContracts(radio(eta_tx=0.0));
end RadioInvalidEfficiency;
model LoadInvalidHysteresis
  extends StoragePowerContracts(camera(voltage_off_v=20.0,voltage_on_v=19.0));
end LoadInvalidHysteresis;
model CameraInvalidPower
  extends StoragePowerContracts(camera(p_active_w=0.1,p_idle_w=0.2));
end CameraInvalidPower;
model RadioNegativePower
  extends StoragePowerContracts(radio(p_rf_w=-1.0));
end RadioNegativePower;
model LinkInvalidCapacity
  extends StoragePowerContracts(link(buffer_capacity_bits=0.0));
end LinkInvalidCapacity;
model LinkInvalidTime
  extends StoragePowerContracts(link(buffer_release_time_s=0.0));
end LinkInvalidTime;
