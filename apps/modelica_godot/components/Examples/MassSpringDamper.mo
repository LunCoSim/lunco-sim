model MassSpringDamper
    "Mass-spring-damper oscillator example"
    
    // Components
    Mechanical.Fixed fixed 
        "Ground point" 
        annotation(Placement(transformation(extent={{-80,-10},{-60,10}})));
    
    Mechanical.Spring spring(k=100) 
        "Spring element" 
        annotation(Placement(transformation(extent={{-40,-10},{-20,10}})));
    
    Mechanical.Mass mass(
        m=1,
        s_start=0.5,
        v_start=0
    ) "Mass element" 
        annotation(Placement(transformation(extent={{0,-10},{20,10}})));
    
    Mechanical.Damper damper(d=2) 
        "Damping element" 
        annotation(Placement(transformation(extent={{40,-10},{60,10}})));
    
    Mechanical.Fixed fixed2 
        "Second ground point" 
        annotation(Placement(transformation(extent={{80,-10},{100,10}})));

equation
    // Connect components
    connect(fixed.flange, spring.flange_a);
    connect(spring.flange_b, mass.flange_a);
    connect(mass.flange_b, damper.flange_a);
    connect(damper.flange_b, fixed2.flange);
    
annotation(
    Documentation(info="<html>
        <p>This model demonstrates a classic mass-spring-damper system.</p>
        <p>The mass is connected to ground through a spring on one side
           and through a damper on the other side.</p>
        <p>Initial conditions:</p>
        <ul>
            <li>Position = 0.5 m (stretched spring)</li>
            <li>Velocity = 0 m/s (starting from rest)</li>
        </ul>
    </html>"),
    experiment(
        StopTime=10,
        Interval=0.01
    ),
    Icon(coordinateSystem(preserveAspectRatio=true))
);
end MassSpringDamper; 