within LunCo.Thermal;
// Causal-to-acausal bridge for a measured dissipation power. `heat_w` comes
// from a physical component's loss output; the heat port then participates in
// the same conservation equations as every thermal mass and radiator.
model HeatLoad
  extends LunCo.Icons.HeatLoad;
  input Real heat_w "Dissipated power received from the physical component, W";
  HeatPort port "Thermal node receiving the dissipated heat";
equation
  // Positive Q enters a component. A loss source supplies the node, hence the
  // negative flow. Clamp numerical noise rather than creating a refrigerator.
  port.Q = -max(0.0, heat_w);
end HeatLoad;
