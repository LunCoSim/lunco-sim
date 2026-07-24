within LunCo.Thermal;
// Acausal thermal connector: temperature T and heat flow rate Q.
// Q > 0: heat flows into the connected component.
// Kirchhoff's Thermal Equilibrium (\sum Q = 0) is enforced at every node.
connector HeatPort
  Real T "Node temperature, K";
  flow Real Q "Heat flow rate entering node, W (summed to zero at node)";
end HeatPort;
