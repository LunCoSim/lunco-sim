within LunCo.Geometry;

// Algebraic measurements for a directed 3-D segment. The endpoints are
// supplied as typed values by the Rhai SysML-to-Modelica adapter; the model
// derives length, midpoint, and orientation axis without owning any USD geometry.
model Segment3D
  parameter Real point_a[3](each unit="m") = {0.0, 0.0, 0.0}
    "First endpoint in the shared design frame";
  parameter Real point_b[3](each unit="m") = {0.0, 0.0, 1.0}
    "Second endpoint in the shared design frame";
  output Real length(unit="m") "Euclidean endpoint separation";
  output Real midpoint[3](each unit="m") "Segment midpoint in the shared design frame";
  output Real axis[3] "Unit vector from the first endpoint to the second";
equation
  length = sqrt((point_b[1] - point_a[1])^2
              + (point_b[2] - point_a[2])^2
              + (point_b[3] - point_a[3])^2);
  midpoint[1] = (point_a[1] + point_b[1]) / 2.0;
  midpoint[2] = (point_a[2] + point_b[2]) / 2.0;
  midpoint[3] = (point_a[3] + point_b[3]) / 2.0;
  assert(length > 0.0, "Segment3D endpoints must be distinct");
  axis[1] = (point_b[1] - point_a[1]) / length;
  axis[2] = (point_b[2] - point_a[2]) / length;
  axis[3] = (point_b[3] - point_a[3]) / length;
end Segment3D;
