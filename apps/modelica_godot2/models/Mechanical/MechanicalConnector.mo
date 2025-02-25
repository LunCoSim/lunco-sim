within ModelicaGodot.Mechanical;
connector MechanicalConnector "Connector for mechanical components with force and position"
  flow Real force "Force transmitted through the connector (N)";
  Real position "Position of the connector (m)";
  Real velocity "Velocity of the connector (m/s)";
  
  annotation(
    Documentation(info="Mechanical connector for 1D translational mechanics.
    - force: Flow variable - Force transmitted through the connector (positive = pushing)
    - position: Potential variable - Absolute position of the connector
    - velocity: Potential variable - Velocity of the connector")
  );
end MechanicalConnector; 