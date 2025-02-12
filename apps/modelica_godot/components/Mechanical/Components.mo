package Mechanical
    "Components for 1D translational mechanical systems"
    
    model Spring
        "Linear 1D translational spring"
        
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
            <p>This component represents a linear 1D translational spring.</p>
            <p>The spring connects two flanges and has a linear characteristic.</p>
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

    model Mass
        "Sliding mass with inertia"
        
        // Connectors
        Interfaces.Flange_a flange_a 
            "Left flange of the mass";
        Interfaces.Flange_b flange_b 
            "Right flange of the mass";
        
        // Parameters
        parameter Real m(unit="kg", min=0) = 1.0 
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

    model Damper
        "Linear 1D translational damper"
        
        // Connectors
        Interfaces.Flange_a flange_a 
            "Left flange of the damper";
        Interfaces.Flange_b flange_b 
            "Right flange of the damper";
        
        // Parameters
        parameter Real d(unit="N.s/m", min=0) = 1.0 
            "Damping constant";
        
        // Variables
        Real v_rel(unit="m/s") 
            "Relative velocity";
        Real f(unit="N") 
            "Damping force";

    equation
        // Relative velocity
        v_rel = der(flange_b.s - flange_a.s);
        
        // Force law
        f = d*v_rel;
        flange_b.f = f;
        flange_a.f = -f;
        
    annotation(
        Documentation(info="<html>
            <p>This component represents a linear damper.</p>
            <p>The damping force is proportional to the relative velocity.</p>
        </html>"),
        Icon(
            coordinateSystem(preserveAspectRatio=true),
            graphics={
                Line(points={{-60,0},{-50,0}}),
                Rectangle(
                    extent={{-50,-30},{50,30}},
                    lineColor={0,0,0},
                    fillColor={192,192,192},
                    fillPattern=FillPattern.Solid
                ),
                Line(points={{50,0},{60,0}})
            }
        )
    );
    end Damper;

    model Fixed
        "Fixed position (ground)"
        
        // Connector
        Interfaces.Flange_a flange 
            "Flange fixed in the ground";
        
        // Parameter
        parameter Real s0 = 0 
            "Fixed offset position";

    equation
        flange.s = s0;
        
    annotation(
        Documentation(info="<html>
            <p>This component represents a fixed position in the inertial system.</p>
            <p>It can be used as a reference point for other components.</p>
        </html>"),
        Icon(
            coordinateSystem(preserveAspectRatio=true),
            graphics={
                Line(points={{-90,0},{0,0}}),
                Line(points={{0,80},{0,-80}}),
                Line(points={{-40,-80},{40,-80}})
            }
        )
    );
    end Fixed;

    model Force
        "Force acting on a flange"
        
        // Connector
        Interfaces.Flange_b flange 
            "Flange where force is applied";
        
        // Input
        input Real f(unit="N") = 0.0 
            "Applied force (positive = push)";

    equation
        flange.f = f;
        
    annotation(
        Documentation(info="<html>
            <p>This component represents an ideal force acting on a flange.</p>
            <p>The force can be specified as an input signal.</p>
        </html>"),
        Icon(
            coordinateSystem(preserveAspectRatio=true),
            graphics={
                Polygon(
                    points={{-100,0},{0,60},{0,-60},{-100,0}},
                    lineColor={0,0,0},
                    fillColor={192,192,192},
                    fillPattern=FillPattern.Solid
                ),
                Line(points={{0,0},{100,0}})
            }
        )
    );
    end Force;

end Mechanical; 