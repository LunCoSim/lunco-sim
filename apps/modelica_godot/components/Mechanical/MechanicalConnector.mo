within Mechanical;
connector MechanicalConnector "Connector for mechanical components with force and position"
  Real force "Force transmitted through the connector (N)";
  Real position "Position of the connector (m)";
  Real velocity "Velocity of the connector (m/s)";
  
  annotation(
    Documentation(info="Mechanical connector for 1D translational mechanics.
    - force: Force transmitted through the connector (positive = pushing)
    - position: Absolute position of the connector
    - velocity: Velocity of the connector")
  );
end MechanicalConnector; 