within LunCo;
package Mobility "Surface mobility: authored command laws and mechanical port demands"
  annotation(
    Documentation(info="<html>
<p>The drive laws map a driver's command ports (throttle and steer) onto the
mechanical demands consumed by a rover's wheels and joints. Each vehicle
composition chooses the model that matches its kinematics.</p>

<p><b>What belongs here:</b> command laws, steering geometry, and motor
dynamics. These are per-vehicle decisions: a skid rover and an Ackermann
rover answer the same command differently, so each has an authored model with
its own equations.</p>

<p><b>What does not belong here:</b> contact acquisition and wheel-force
realization. Those are generic USD/Avian and raycast mechanisms. The outputs
of these models are solved mechanical port values, not a second vehicle
implementation in Rust.</p>
</html>")
  );
end Mobility;
