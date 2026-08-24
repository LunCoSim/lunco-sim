within LunCo;
package Mechanics "Rotational mechanics: the domain that joins a motor to a wheel"
  annotation(
    Documentation(info="<html>
<p>The rotational counterpart to <code>Electrical</code> and <code>Thermal</code>. Those two
have had a connector since the beginning — <code>Pin</code> and <code>HeatPort</code> — so a
battery joins a motor, and a motor joins a radiator, acausally: state a shared potential, make
the flow a <code>flow</code>, and Modelica balances the node. The rotational network uses the
same principle through <code>Flange</code>: <code>phi</code> is shared, <code>tau</code> is a
<code>flow</code>, and the torque balance at a node is Newton's third law rather than an
authored convention. Two components joined at a flange agree about speed and torque
simultaneously, in one equation set, with no direction of travel.</p>

<p>The authored motor, reduction, and wheel boundary are one network. The reduction exposes its
fast-side speed from the solved flange, so the motor sees the speed imposed by the wheel through
the authored ratio. The Avian boundary is the only measured-speed input; no Rust torque curve or
copied speed limit participates in the solution.</p>

<p><b>Scope.</b> This package owns the DRIVELINE — everything between the motor's electrical
terminals and the wheel hub. It does NOT own the contact patch. Tyre force, the friction cone
and the wheel's rigid-body state belong to Avian, because they are contact quantities the
solver resolves against the ground each step, and a co-simulated tyre would be explicitly
coupled across a stiff boundary. See <code>Electrical</code>'s note on the same division: this
package owns what the physics engine has no opinion about.</p>
</html>")
  );
end Mechanics;
