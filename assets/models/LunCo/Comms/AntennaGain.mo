within LunCo.Comms;
// Antenna gain model: directional antenna gain as a function of pointing elevation angle.
model AntennaGain
  parameter Real gain_max_dbi = 18.0 "Peak bore-sight antenna gain, dBi";
  parameter Real min_elevation_deg = 5.0 "Minimum elevation mask angle, deg";

  input Real elevation_deg "Target elevation angle, deg";
  output Real gain_dbi "Effective antenna gain, dBi";
  output Real connected "Link connected state (1.0 = visible above mask, 0.0 = masked)";
equation
  connected = max(0.0, min(1.0, (elevation_deg - min_elevation_deg) * 10.0));
  gain_dbi = -30.0 + connected * (gain_max_dbi + 30.0);
end AntennaGain;
