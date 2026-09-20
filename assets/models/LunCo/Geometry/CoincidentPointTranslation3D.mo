within LunCo.Geometry;

// Solve the translation that makes two points coincide in one shared frame.
// Orientation is assumed fixed; this relation does not solve rotation.
model CoincidentPointTranslation3D
  parameter Real moving_point[3](each unit="m")
    "Current component-side point in the shared design frame";
  parameter Real fixed_point[3](each unit="m")
    "Target datum point in the shared design frame";
  output Real translation[3](each unit="m")
    "Translation to apply to the moving component";
equation
  for axis in 1:3 loop
    moving_point[axis] + translation[axis] = fixed_point[axis];
  end for;
end CoincidentPointTranslation3D;
