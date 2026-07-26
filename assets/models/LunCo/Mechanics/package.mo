within LunCo;
package Mechanics "Rotational mechanics: the domain that joins a motor to a wheel"
  annotation(
    Documentation(info="<html>
<p>The rotational counterpart to <code>Electrical</code> and <code>Thermal</code>. Those two
have had a connector since the beginning — <code>Pin</code> and <code>HeatPort</code> — so a
battery joins a motor, and a motor joins a radiator, acausally: state a shared potential, make
the flow a <code>flow</code>, and Modelica balances the node. There was no such connector for
torque, which is why the one coupling every rover actually depends on — motor to gearbox to
wheel — was the one that had to be faked.</p>

<p>Faked as a CAUSAL wire: <code>Gearbox.inputs:torque.connect = Motor.outputs:torque</code>.
That spelling carries torque in one direction and cannot carry speed back, so the gearbox never
tells the motor how fast it is being turned. A motor that cannot see its own shaft speed cannot
produce a torque-speed curve — which is precisely why the curve had to live outside the model,
in Rust, and why a phantom wire, a suppression rule for it, and a misdiagnosis all grew up
around the gap.</p>

<p>A <code>Flange</code> fixes that by construction: <code>phi</code> is shared, <code>tau</code>
is a <code>flow</code>, and the torque balance at a node is Newton's third law rather than an
authored convention. Two components joined at a flange agree about speed and torque
simultaneously, in one equation set, with no direction of travel.</p>

<p><b>Scope.</b> This package owns the DRIVELINE — everything between the motor's electrical
terminals and the wheel hub. It does NOT own the contact patch. Tyre force, the friction cone
and the wheel's rigid-body state belong to Avian, because they are contact quantities the
solver resolves against the ground each step, and a co-simulated tyre would be explicitly
coupled across a stiff boundary. See <code>Electrical</code>'s note on the same division: this
package owns what the physics engine has no opinion about.</p>
</html>")
  );
end Mechanics;
