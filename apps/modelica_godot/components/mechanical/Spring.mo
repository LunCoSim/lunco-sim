model Spring
    "1D translational spring"
    
    // Connectors
    Interfaces.Flange_a flange_a 
        "Left flange of the spring";
    Interfaces.Flange_b flange_b 
        "Right flange of the spring";
    
    // Parameters
    parameter Real k(unit="N/m", min=0) = 1.0 
        "Spring constant";
    parameter Real s_rel0 = 0 
        "Unstretched spring length";
    
    // Variables
    Real s_rel(unit="m") 
        "Spring elongation = flange_b.s - flange_a.s";
    Real f(unit="N") 
        "Spring force";

equation
    // Position difference
    s_rel = flange_b.s - flange_a.s;
    
    // Force balance
    f = k*(s_rel - s_rel0);
    flange_b.f = f;
    flange_a.f = -f;
    
annotation(
    Documentation(info="<html>
        <p>This component represents a linear 1D translational spring</p>
        <p>The spring connects two flanges and has a linear characteristic.</p>
        <p>Parameters:</p>
        <ul>
            <li>k: Spring constant [N/m]</li>
            <li>s_rel0: Unstretched length [m]</li>
        </ul>
    </html>"),
    Icon(
        coordinateSystem(preserveAspectRatio=true),
        graphics={
            Line(points={{-60,0},{-50,0}}),
            Line(points={{50,0},{60,0}}),
            Line(points={{-50,0},{-30,20},{-10,-20},{10,20},{30,-20},{50,0}})
        }
    )
);
end Spring; 