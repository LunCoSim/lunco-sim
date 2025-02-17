within ModelicaGodot;
package Mechanical "Basic mechanical components library"
  annotation(
    Documentation(info="Basic mechanical components for 1D translational mechanics.
    Components:
    - MechanicalConnector: Basic connector with force and position
    - Mass: Point mass with one connector
    - Spring: Linear spring following Hooke's law
    - Fixed: Fixed point in space (ground)")
  );
end Mechanical; 