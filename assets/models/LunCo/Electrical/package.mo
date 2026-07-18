within LunCo;
package Electrical "Electrical power: what makes it, what stores it, what draws it"
  annotation(Documentation(info = "<html>
<p>The electrical power system. Sources (<code>SolarPanel</code>), storage
(<code>Battery</code>) and loads (<code>DCMotor</code>) — each an equation, so each a
model. A quantity with an equation behind it can be checked; an authored attribute with
none can only be trusted.</p>

<p>The MECHANICAL side of these parts stays in USD, where Avian reads it: a motor's torque
reaches a wheel through the PhysX vehicle properties and the physics step. This package
owns only what the physics engine has no opinion about — what the motor DRAWS to hold that
torque.</p>
</html>"));
end Electrical;
