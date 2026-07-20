within LunCo;
package Propulsion "Engines: what a nozzle's geometry is worth, and what it burns"
  annotation(Documentation(info = "<html>
<p>The equations behind the thrust. USD authors an engine's DESIGN — throat and exit
radii, contour, chamber pressure — and this package turns that design into the numbers
that follow from it: expansion ratio, thrust coefficient, specific impulse, thrust.</p>

<p>The split matters. Geometry is a vehicle decision and lives in USD, where it is a
parameter anyone can change. Its consequences are physics and live here, where they are
equations that can be checked. A number typed into a script is neither.</p>

<p>Nothing here runs per frame: a nozzle's shape does not change while the engine burns.
These are design quantities, evaluated from their inputs — which is also why the shape
math does not belong in a rhai actuator ticking 25 times a second.</p>
</html>"));
end Propulsion;
