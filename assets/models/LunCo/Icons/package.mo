within LunCo;
// Canonical vector icons for LunCo's reusable Modelica components. Models
// inherit these annotations so assemblies keep one visual vocabulary.
package Icons "Semantic icons for the LunCo Modelica library"
  partial model Battery
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Rectangle(extent={{-70,-45},{70,45}}, lineColor={35,85,150}, fillColor={95,155,220}, fillPattern=FillPattern.Solid, radius=10),
      Line(points={{-18,0},{18,0}}, color={255,255,255}, thickness=1), Line(points={{0,-18},{0,18}}, color={255,255,255}, thickness=1), Line(points={{42,-12},{62,-12}}, color={255,255,255}, thickness=1),
      Text(extent={{-80,-78},{80,-54}}, textString="BATTERY", textColor={35,85,150}, fontSize=10)}));
  end Battery;
  partial model SolarPanel
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Polygon(points={{-78,-42},{45,-65},{72,35},{-50,60},{-78,-42}}, lineColor={20,75,145}, fillColor={55,130,205}, fillPattern=FillPattern.Solid),
      Line(points={{-55,-32},{58,-53}}, color={200,230,255}, thickness=1), Line(points={{-42,10},{65,-10}}, color={200,230,255}, thickness=1), Line(points={{-18,49},{-2,-53}}, color={200,230,255}, thickness=1), Line(points={{24,42},{39,-60}}, color={200,230,255}, thickness=1),
      Ellipse(extent={{52,48},{86,82}}, lineColor={235,165,20}, fillColor={255,205,55}, fillPattern=FillPattern.Solid), Text(extent={{-92,-90},{92,-70}}, textString="SOLAR PANEL", textColor={20,75,145}, fontSize=9)}));
  end SolarPanel;
  partial model Motor
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Rectangle(extent={{-62,-46},{62,46}}, lineColor={115,65,25}, fillColor={235,155,65}, fillPattern=FillPattern.Solid, radius=14),
      Ellipse(extent={{-34,-34},{34,34}}, lineColor={115,65,25}, fillColor={255,210,125}, fillPattern=FillPattern.Solid),
      Ellipse(extent={{-13,-13},{13,13}}, lineColor={115,65,25}, fillColor={150,82,30}, fillPattern=FillPattern.Solid),
      Line(points={{-4,-27},{4,-27},{4,27},{-4,27},{-4,-27}}, color={115,65,25}, thickness=2),
      Line(points={{-90,0},{-62,0}}, color={115,65,25}, thickness=2), Line(points={{62,0},{90,0}}, color={115,65,25}, thickness=2),
      Text(extent={{-82,-88},{82,-68}}, textString="%name", textColor={115,65,25}, fontSize=10)}));
  end Motor;
  partial model ElectricalControl
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Rectangle(extent={{-70,-60},{70,60}}, lineColor={100,65,145}, fillColor={180,145,215}, fillPattern=FillPattern.Solid, radius=8), Rectangle(extent={{-35,-25},{35,25}}, lineColor={255,255,255}, fillColor={75,55,110}, fillPattern=FillPattern.Solid, radius=3),
      Line(points={{-88,-35},{-70,-35}}, color={100,65,145}, thickness=1), Line(points={{-88,0},{-70,0}}, color={100,65,145}, thickness=1), Line(points={{-88,35},{-70,35}}, color={100,65,145}, thickness=1), Line(points={{70,-35},{88,-35}}, color={100,65,145}, thickness=1), Line(points={{70,0},{88,0}}, color={100,65,145}, thickness=1), Line(points={{70,35},{88,35}}, color={100,65,145}, thickness=1)}));
  end ElectricalControl;
  partial model Camera
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Rectangle(extent={{-72,-48},{42,48}}, lineColor={55,70,95}, fillColor={125,155,190}, fillPattern=FillPattern.Solid, radius=8), Polygon(points={{42,-28},{82,-48},{82,48},{42,28},{42,-28}}, lineColor={55,70,95}, fillColor={90,120,160}, fillPattern=FillPattern.Solid),
      Ellipse(extent={{-30,-28},{26,28}}, lineColor={25,35,50}, fillColor={45,65,100}, fillPattern=FillPattern.Solid), Ellipse(extent={{-12,-10},{8,10}}, lineColor={180,225,255}, fillColor={180,225,255}, fillPattern=FillPattern.Solid)}));
  end Camera;
  partial connector ElectricalPin
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Ellipse(extent={{-100,-100},{100,100}}, lineColor={25,80,155}, fillColor={90,160,230}, fillPattern=FillPattern.Solid), Text(extent={{-60,-35},{60,35}}, textString="V", textColor={255,255,255}, fontSize=28)}));
  end ElectricalPin;
  partial connector DataPort
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Polygon(points={{0,100},{100,0},{0,-100},{-100,0},{0,100}}, lineColor={20,125,140}, fillColor={65,195,205}, fillPattern=FillPattern.Solid), Text(extent={{-55,-30},{55,30}}, textString="D", textColor={0,70,80}, fontSize=24)}));
  end DataPort;
  partial model Comms
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Ellipse(extent={{-68,-68},{68,68}}, lineColor={30,125,150}, fillColor={110,205,220}, fillPattern=FillPattern.Solid), Polygon(points={{-20,-22},{30,0},{-20,22},{-20,-22}}, lineColor={0,75,95}, fillColor={230,250,250}, fillPattern=FillPattern.Solid), Line(points={{38,-42},{66,0},{38,42}}, color={0,75,95}, thickness=1), Line(points={{56,-60},{90,0},{56,60}}, color={0,75,95}, thickness=1)}));
  end Comms;
  partial model DataBuffer
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-70,-55},{70,55}}, lineColor={20,115,135}, fillColor={105,205,210}, fillPattern=FillPattern.Solid, radius=6), Rectangle(extent={{-45,16},{45,34}}, lineColor={235,255,255}, fillColor={235,255,255}, fillPattern=FillPattern.Solid), Rectangle(extent={{-45,-9},{20,9}}, lineColor={235,255,255}, fillColor={235,255,255}, fillPattern=FillPattern.Solid), Rectangle(extent={{-45,-34},{-5,-16}}, lineColor={235,255,255}, fillColor={235,255,255}, fillPattern=FillPattern.Solid)}));
  end DataBuffer;
  partial connector HeatPort
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Ellipse(extent={{-100,-100},{100,100}}, lineColor={185,70,20}, fillColor={245,135,55}, fillPattern=FillPattern.Solid), Line(points={{-25,-55},{5,-10},{-12,-10},{25,55},{-5,12},{12,12},{-25,-55}}, color={255,245,210}, thickness=1)}));
  end HeatPort;
  partial model HeatLoad
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-70,-55},{70,55}}, lineColor={180,55,25}, fillColor={235,105,65}, fillPattern=FillPattern.Solid, radius=8), Line(points={{-48,0},{-25,25},{-5,-25},{18,25},{45,-25}}, color={255,245,220}, thickness=2), Text(extent={{-70,-86},{70,-65}}, textString="HEAT LOAD", textColor={180,55,25}, fontSize=9)}));
  end HeatLoad;
  partial model ThermalMass
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-62,-62},{62,62}}, lineColor={175,75,20}, fillColor={245,165,70}, fillPattern=FillPattern.Solid, radius=12), Line(points={{0,-40},{0,28}}, color={255,255,245}, thickness=2), Ellipse(extent={{-18,-52},{18,-16}}, lineColor={255,255,245}, fillColor={220,65,45}, fillPattern=FillPattern.Solid), Line(points={{-18,30},{18,30}}, color={255,255,245}, thickness=2)}));
  end ThermalMass;
  partial model Radiator
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-62,-48},{18,48}}, lineColor={35,115,155}, fillColor={120,190,220}, fillPattern=FillPattern.Solid), Line(points={{-42,-45},{-42,45}}, color={230,250,255}, thickness=1), Line(points={{-20,-45},{-20,45}}, color={230,250,255}, thickness=1), Line(points={{28,-35},{78,-55}}, color={220,75,35}, thickness=1), Line(points={{28,0},{88,0}}, color={220,75,35}, thickness=1), Line(points={{28,35},{78,55}}, color={220,75,35}, thickness=1)}));
  end Radiator;
  partial model ThermalConductor
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-78,-24},{78,24}}, lineColor={175,75,20}, fillColor={245,165,70}, fillPattern=FillPattern.Solid, radius=8), Line(points={{-95,0},{-78,0}}, color={175,75,20}, thickness=2), Line(points={{78,0},{95,0}}, color={175,75,20}, thickness=2), Text(extent={{-60,-12},{60,12}}, textString="THERMAL", textColor={110,50,15}, fontSize=11)}));
  end ThermalConductor;
  partial model Heater
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-65,-55},{65,55}}, lineColor={180,50,20}, fillColor={245,115,55}, fillPattern=FillPattern.Solid, radius=8), Line(points={{-45,-20},{-22,20},{0,-20},{22,20},{45,-20}}, color={255,250,225}, thickness=2), Text(extent={{-55,-10},{55,12}}, textString="HEAT", textColor={130,35,15}, fontSize=13)}));
  end Heater;
  partial connector Flange
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Ellipse(extent={{-100,-100},{100,100}}, lineColor={85,85,90}, fillColor={210,210,215}, fillPattern=FillPattern.Solid), Ellipse(extent={{-42,-42},{42,42}}, lineColor={85,85,90}, fillColor={95,95,100}, fillPattern=FillPattern.Solid)}));
  end Flange;
  partial model Mechanics
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Ellipse(extent={{-66,-66},{66,66}}, lineColor={70,70,75}, fillColor={180,185,190}, fillPattern=FillPattern.Solid), Ellipse(extent={{-25,-25},{25,25}}, lineColor={70,70,75}, fillColor={245,245,245}, fillPattern=FillPattern.Solid), Line(points={{-90,0},{90,0}}, color={70,70,75}, thickness=1)}));
  end Mechanics;
  partial model Mobility
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-70,-18},{70,35}}, lineColor={70,95,65}, fillColor={135,165,100}, fillPattern=FillPattern.Solid, radius=8), Ellipse(extent={{-62,-62},{-15,-15}}, lineColor={45,45,45}, fillColor={70,70,70}, fillPattern=FillPattern.Solid), Ellipse(extent={{15,-62},{62,-15}}, lineColor={45,45,45}, fillColor={70,70,70}, fillPattern=FillPattern.Solid), Line(points={{-92,-65},{92,-65}}, color={125,105,75}, thickness=1)}));
  end Mobility;
  partial model Pointing
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Ellipse(extent={{-62,-62},{62,62}}, lineColor={35,115,150}, fillColor={125,195,225}, fillPattern=FillPattern.Solid), Line(points={{0,-82},{0,82}}, color={20,75,110}, thickness=1), Line(points={{-82,0},{82,0}}, color={20,75,110}, thickness=1), Ellipse(extent={{-16,-16},{16,16}}, lineColor={255,255,255}, fillColor={255,255,255}, fillPattern=FillPattern.Solid)}));
  end Pointing;
  partial model Propulsion
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Polygon(points={{-58,50},{48,25},{72,-25},{-58,-50},{-25,0},{-58,50}}, lineColor={105,105,115}, fillColor={185,190,205}, fillPattern=FillPattern.Solid), Polygon(points={{-58,30},{-95,0},{-58,-30},{-38,0},{-58,30}}, lineColor={220,75,25}, fillColor={250,150,45}, fillPattern=FillPattern.Solid)}));
  end Propulsion;
  partial model RCSJet
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Polygon(points={{-58,42},{42,25},{66,0},{42,-25},{-58,-42},{-28,0},{-58,42}}, lineColor={95,95,105}, fillColor={180,185,200}, fillPattern=FillPattern.Solid),
      Polygon(points={{-58,26},{-96,0},{-58,-26},{-36,0},{-58,26}}, lineColor={220,70,20}, fillColor={255,150,35}, fillPattern=FillPattern.Solid),
      Line(points={{-86,0},{-100,0}}, color={255,225,125}, thickness=2),
      Text(extent={{-88,-88},{88,-68}}, textString=DynamicSelect("RCS", "RCS " + String(activity)), textColor={160,65,20}, fontSize=10)}));
  end RCSJet;
  partial model RCSThruster
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Rectangle(extent={{-58,34},{52,-34}}, lineColor={95,95,105}, fillColor={180,185,200}, fillPattern=FillPattern.Solid, radius=7),
      Polygon(points={{52,25},{92,0},{52,-25},{52,25}}, lineColor={220,70,20}, fillColor={255,150,35}, fillPattern=FillPattern.Solid),
      Line(points={{-88,0},{-58,0}}, color={95,95,105}, thickness=2),
      Text(extent={{-88,-88},{88,-68}}, textString=DynamicSelect("THRUSTER", "THRUSTER " + String(thrust_n) + " N"), textColor={160,65,20}, fontSize=9)}));
  end RCSThruster;
  partial model PropellantTank
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Rectangle(extent={{-58,-60},{58,60}}, lineColor={45,85,135}, fillColor={105,165,220}, fillPattern=FillPattern.Solid, radius=16),
      Ellipse(extent={{-58,38},{58,76}}, lineColor={45,85,135}, fillColor={145,200,240}, fillPattern=FillPattern.Solid),
      Line(points={{-35,-22},{35,-22}}, color={225,245,255}, thickness=2),
      Line(points={{-35,10},{35,10}}, color={225,245,255}, thickness=2),
      Line(points={{0,-78},{0,-60}}, color={45,85,135}, thickness=2),
      Text(extent={{-85,-92},{85,-74}}, textString=DynamicSelect("TANK", "TANK " + String(mass_kg) + " kg"), textColor={45,85,135}, fontSize=10)}));
  end PropellantTank;
  partial model Turbopump
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Ellipse(extent={{-62,-62},{62,62}}, lineColor={105,65,30}, fillColor={225,155,65}, fillPattern=FillPattern.Solid),
      Ellipse(extent={{-25,-25},{25,25}}, lineColor={105,65,30}, fillColor={245,220,150}, fillPattern=FillPattern.Solid),
      Line(points={{-92,0},{-62,0}}, color={105,65,30}, thickness=2),
      Line(points={{62,0},{92,0}}, color={105,65,30}, thickness=2),
      Polygon(points={{-18,0},{12,18},{12,-18},{-18,0}}, lineColor={105,65,30}, fillColor={180,90,35}, fillPattern=FillPattern.Solid),
      Text(extent={{-88,-92},{88,-74}}, textString=DynamicSelect("PUMP", "PUMP " + String(speed_fraction)), textColor={105,65,30}, fontSize=10)}));
  end Turbopump;
  partial model CombustionChamber
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Polygon(points={{-55,55},{38,38},{72,0},{38,-38},{-55,-55},{-22,0},{-55,55}}, lineColor={130,55,20}, fillColor={225,100,45}, fillPattern=FillPattern.Solid),
      Polygon(points={{-55,25},{-92,0},{-55,-25},{-35,0},{-55,25}}, lineColor={210,70,20}, fillColor={255,175,55}, fillPattern=FillPattern.Solid),
      Line(points={{38,0},{92,0}}, color={110,110,120}, thickness=2),
      Text(extent={{-95,-92},{95,-74}}, textString=DynamicSelect("CHAMBER", "CHAMBER " + String(activity)), textColor={130,55,20}, fontSize=9)}));
  end CombustionChamber;
  partial model PropellantStatus
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
      Rectangle(extent={{-72,-58},{72,58}}, lineColor={85,70,130}, fillColor={175,155,220}, fillPattern=FillPattern.Solid, radius=10),
      Line(points={{-48,-25},{-48,28}}, color={245,245,255}, thickness=3),
      Line(points={{-48,-25},{35,-25}}, color={245,245,255}, thickness=3),
      Line(points={{35,-25},{35,25}}, color={245,245,255}, thickness=3),
      Text(extent={{-88,-90},{88,-70}}, textString="STATUS", textColor={85,70,130}, fontSize=10)}));
  end PropellantStatus;
  partial model Sensor
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-60,-60},{60,60}}, lineColor={30,120,145}, fillColor={105,200,215}, fillPattern=FillPattern.Solid, radius=10), Ellipse(extent={{-30,-30},{30,30}}, lineColor={235,255,255}, fillColor={35,105,135}, fillPattern=FillPattern.Solid), Line(points={{0,65},{0,88}}, color={30,120,145}, thickness=1), Line(points={{0,-65},{0,-88}}, color={30,120,145}, thickness=1)}));
  end Sensor;
  partial model Storage
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-60,-62},{60,62}}, lineColor={55,105,155}, fillColor={115,170,220}, fillPattern=FillPattern.Solid, radius=18), Ellipse(extent={{-60,42},{60,78}}, lineColor={55,105,155}, fillColor={145,195,235}, fillPattern=FillPattern.Solid), Line(points={{-48,-20},{48,-20}}, color={230,245,255}, thickness=1), Line(points={{-48,12},{48,12}}, color={230,245,255}, thickness=1)}));
  end Storage;
  partial model Guidance
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Polygon(points={{-70,-52},{72,0},{-70,52},{-34,0},{-70,-52}}, lineColor={70,80,150}, fillColor={130,145,225}, fillPattern=FillPattern.Solid), Line(points={{-55,0},{36,0}}, color={255,255,255}, thickness=2)}));
  end Guidance;
  partial block Logic
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Polygon(points={{-75,0},{0,65},{75,0},{0,-65},{-75,0}}, lineColor={100,65,145}, fillColor={190,150,220}, fillPattern=FillPattern.Solid), Text(extent={{-55,-24},{55,24}}, textString=">", textColor={255,255,255}, fontSize=35)}));
  end Logic;
end Icons;
