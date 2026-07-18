within LunCo.Electrical;
// The electrical connector. `v` is shared at a node (all pins tied together see one
// voltage); `i` is a `flow`, so Modelica sums the currents at every node to zero — that
// is Kirchhoff's current law, and it is why a bus needs no count of what is on it.
connector Pin
  Real v "Node voltage, V";
  flow Real i "Current INTO the pin, A";
end Pin;
