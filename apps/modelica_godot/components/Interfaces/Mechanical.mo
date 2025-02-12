package Interfaces
    "Basic interface definitions for physical domains"

    connector Flange
        "Mechanical flange connector (translational 1D)"
        
        Real s(unit="m") 
            "Position";
        flow Real f(unit="N") 
            "Force";
            
        annotation(
            Documentation(info="<html>
                <p>Basic mechanical flange for 1D translational mechanics.</p>
                <p>Contains position (s) as potential variable and force (f) as flow variable.</p>
            </html>")
        );
    end Flange;

    connector Flange_a
        "Left mechanical flange connector (translational 1D)"
        extends Flange;
        
        annotation(
            Documentation(info="<html>
                <p>Left flange of a 1D translational mechanical component.</p>
            </html>"),
            Icon(graphics={
                Rectangle(
                    extent={{-10,10},{10,-10}},
                    lineColor={0,0,0},
                    fillColor={0,0,0},
                    fillPattern=FillPattern.Solid
                )
            })
        );
    end Flange_a;

    connector Flange_b
        "Right mechanical flange connector (translational 1D)"
        extends Flange;
        
        annotation(
            Documentation(info="<html>
                <p>Right flange of a 1D translational mechanical component.</p>
            </html>"),
            Icon(graphics={
                Rectangle(
                    extent={{-10,10},{10,-10}},
                    lineColor={0,0,0},
                    fillColor={255,255,255},
                    fillPattern=FillPattern.Solid
                )
            })
        );
    end Flange_b;
end Interfaces; 