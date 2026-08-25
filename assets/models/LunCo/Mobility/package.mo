within LunCo;
package Mobility "Surface mobility: how a driver's command becomes per-wheel demand"
  annotation(
    Documentation(info="<html>
<p>The drive LAWS — the mapping from a driver's two numbers (throttle, steer) onto what each
side or axle is asked to do. Every rover in the fleet used one of these, and until now every
one of them lived in the flat pile beside the teaching examples, unable to compose with the
library that models their physics.</p>

<p><b>What belongs here:</b> command mixing, steering geometry, and the lags that make a
command build rather than step. These are per-VEHICLE decisions — a skid rover and an
Ackermann rover answer the same command differently, which is exactly why they are two models
and not one parameterised one. The target layout sketched a single merged
<code>DriveMixer</code>; that was not adopted, because the two laws differ in their equations
and not merely in their constants, and collapsing them would trade a real distinction for a
smaller file count.</p>

<p><b>What does not belong here:</b> the torque a motor can actually produce (that is
the native USD/Avian motor powertrain), the reduction between motor and wheel, and anything
about the contact patch, which is Avian's. <code>Electrical.DCMotor</code> is the electrical
load facet of that native powertrain; it does not own its shaft torque or speed.
The output of a model in this package is a NORMALISED demand, not a torque — it says what the
driver asked of each side, and the driveline decides what that costs.</p>
</html>")
  );
end Mobility;
