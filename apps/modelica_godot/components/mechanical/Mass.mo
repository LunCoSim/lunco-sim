model Mass
    "Sliding mass with inertia"
    
    // Connectors
    Modelica.Mechanics.Translational.Interfaces.Flange_a flange_a 
        "Left flange of the mass";
    Modelica.Mechanics.Translational.Interfaces.Flange_b flange_b 
        "Right flange of the mass";
    
    // Parameters
    parameter Real m(unit="kg") = 1.0 
        "Mass of the sliding body";
    parameter Real s_start = 0.0 
        "Initial position";
    parameter Real v_start = 0.0 
        "Initial velocity";
    
    // Variables
    Real s(unit="m", start=s_start) 
        "Position of center of mass";
    Real v(unit="m/s", start=v_start) 
        "Velocity of mass";
    Real a(unit="m/s2") 
        "Acceleration of mass";

equation
    // Define relationship between position and velocity
    der(s) = v;
    
    // Newton's second law
    m*der(v) = flange_a.f + flange_b.f;
    
    // Accelerations
    a = der(v);
    
    // The position of the mass is the same as the position of both flanges
    flange_a.s = s;
    flange_b.s = s;
    
annotation(
    Documentation(info="<html>
        <p>This component represents a sliding mass with inertia.</p>
        <p>The mass can slide along a line and has two flanges for force input.</p>
    </html>"),
    Icon(
        coordinateSystem(preserveAspectRatio=true),
        graphics={
            Rectangle(
                extent={{-30,-30},{30,30}},
                lineColor={0,0,0},
                fillColor={192,192,192},
                fillPattern=FillPattern.Solid
            ),
            Line(points={{-60,0},{-30,0}}),
            Line(points={{30,0},{60,0}})
        }
    )
);
end Mass; 