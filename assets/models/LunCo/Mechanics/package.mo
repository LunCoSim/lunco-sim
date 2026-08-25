within LunCo;
package Mechanics "Standalone rotational driveline models"
  annotation(
    Documentation(info="<html>
<p>The rotational counterpart to <code>Electrical</code> and <code>Thermal</code>. A
<code>Flange</code> states a shared angular potential and a torque flow, so explicitly
composed Modelica driveline models can be balanced by the solver. Production rover
motor torque, reduction, wheel state, and contact remain owned by USD and Avian.</p>

<p>The USD rover powertrain still uses the authored causal
<code>Gearbox.inputs:torque.connect</code> topology, which is read by the native powertrain
projector. It is not silently reinterpreted as a Modelica rotational connection. A Modelica
motor is admitted to a rotational network only when that network authors its complete flange
topology and boundary.</p>

<p>A <code>Flange</code> fixes that by construction: <code>phi</code> is shared, <code>tau</code>
is a <code>flow</code>, and the torque balance at a node is Newton's third law rather than an
authored convention. Two components joined at a flange agree about speed and torque
simultaneously, in one equation set, with no direction of travel.</p>

<p><b>Scope.</b> This package owns reusable Modelica rotational equations only. It does NOT
own the rover contact patch. Tyre force, the friction cone, wheel rigid-body state, and the
production rover powertrain belong to Avian and the USD powertrain projection. A model that
crosses that boundary must author an explicit co-simulation interface rather than declaring a
free flange inside an electrical island.</p>
</html>")
  );
end Mechanics;
