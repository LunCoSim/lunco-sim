within LunCo;
package Actuation
  "Reusable actuator-domain models composed from USD"
  annotation(Documentation(info = "<html>
<p>Actuation models contain reusable equations for converting a requested wrench
into scalar actuator commands. USD owns the actuator instances, transforms,
limits, and connections; this package owns the equations.</p>
</html>"));
end Actuation;
