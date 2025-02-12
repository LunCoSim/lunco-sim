package Electrical
    "Components for electrical systems"
    
    model Resistor
        "Ideal linear electrical resistor"
        
        // Connectors
        Interfaces.PositivePin p 
            "Positive pin";
        Interfaces.NegativePin n 
            "Negative pin";
        
        // Parameters
        parameter Real R(unit="Ohm", min=0) = 1.0 
            "Resistance";
        
        // Variables
        Real v(unit="V") 
            "Voltage drop";
        Real i(unit="A") 
            "Current";
        Real power(unit="W") 
            "Power dissipation";

    equation
        // Voltage drop
        v = p.v - n.v;
        
        // Ohm's law
        v = R*i;
        
        // Current balance
        p.i = i;
        n.i = -i;
        
        // Power calculation
        power = v*i;
        
    annotation(
        Documentation(info="<html>
            <p>This component represents an ideal linear electrical resistor.</p>
            <p>The resistance R is constant and the component is symmetric.</p>
        </html>"),
        Icon(
            coordinateSystem(preserveAspectRatio=true),
            graphics={
                Rectangle(
                    extent={{-60,20},{60,-20}},
                    lineColor={0,0,0},
                    fillColor={255,255,255},
                    fillPattern=FillPattern.Solid
                ),
                Line(points={{-90,0},{-60,0}}),
                Line(points={{60,0},{90,0}})
            }
        )
    );
    end Resistor;

    model VoltageSource
        "Ideal voltage source"
        
        // Connectors
        Interfaces.PositivePin p 
            "Positive pin";
        Interfaces.NegativePin n 
            "Negative pin";
        
        // Parameters
        parameter Real V(unit="V") = 12.0 
            "Source voltage";
        
        // Variables
        Real i(unit="A") 
            "Current";
        Real power(unit="W") 
            "Power output";

    equation
        // Voltage definition
        p.v - n.v = V;
        
        // Current balance
        p.i = i;
        n.i = -i;
        
        // Power calculation
        power = V*i;
        
    annotation(
        Documentation(info="<html>
            <p>This component represents an ideal voltage source.</p>
            <p>The voltage between positive and negative pin is constant.</p>
        </html>"),
        Icon(
            coordinateSystem(preserveAspectRatio=true),
            graphics={
                Ellipse(
                    extent={{-50,50},{50,-50}},
                    lineColor={0,0,0},
                    fillColor={255,255,255},
                    fillPattern=FillPattern.Solid
                ),
                Line(points={{-90,0},{-50,0}}),
                Line(points={{50,0},{90,0}}),
                Text(
                    extent={{-30,30},{30,-30}},
                    textString="V"
                ),
                Line(points={{-20,20},{20,-20}})
            }
        )
    );
    end VoltageSource;

    model Ground
        "Ground node"
        
        // Connector
        Interfaces.PositivePin p 
            "Ground pin";

    equation
        p.v = 0;
        
    annotation(
        Documentation(info="<html>
            <p>This component defines the ground (reference potential) of an electrical circuit.</p>
        </html>"),
        Icon(
            coordinateSystem(preserveAspectRatio=true),
            graphics={
                Line(points={{-60,0},{60,0}}),
                Line(points={{-40,-20},{40,-20}}),
                Line(points={{-20,-40},{20,-40}}),
                Line(points={{0,0},{0,40}})
            }
        )
    );
    end Ground;

    model Capacitor
        "Ideal electrical capacitor"
        
        // Connectors
        Interfaces.PositivePin p 
            "Positive pin";
        Interfaces.NegativePin n 
            "Negative pin";
        
        // Parameters
        parameter Real C(unit="F", min=0) = 1e-6 
            "Capacitance";
        parameter Real v_start = 0 
            "Initial voltage";
        
        // Variables
        Real v(unit="V", start=v_start) 
            "Voltage drop";
        Real i(unit="A") 
            "Current";

    equation
        // Voltage drop
        v = p.v - n.v;
        
        // Basic capacitor equation
        i = C*der(v);
        
        // Current balance
        p.i = i;
        n.i = -i;
        
    annotation(
        Documentation(info="<html>
            <p>This component represents an ideal electrical capacitor.</p>
            <p>The capacitance C is constant.</p>
        </html>"),
        Icon(
            coordinateSystem(preserveAspectRatio=true),
            graphics={
                Line(points={{-90,0},{-14,0}}),
                Line(points={{14,0},{90,0}}),
                Line(points={{-14,28},{-14,-28}}),
                Line(points={{14,28},{14,-28}})
            }
        )
    );
    end Capacitor;

    model Inductor
        "Ideal electrical inductor"
        
        // Connectors
        Interfaces.PositivePin p 
            "Positive pin";
        Interfaces.NegativePin n 
            "Negative pin";
        
        // Parameters
        parameter Real L(unit="H", min=0) = 1e-3 
            "Inductance";
        parameter Real i_start = 0 
            "Initial current";
        
        // Variables
        Real v(unit="V") 
            "Voltage drop";
        Real i(unit="A", start=i_start) 
            "Current";

    equation
        // Voltage drop
        v = p.v - n.v;
        
        // Basic inductor equation
        v = L*der(i);
        
        // Current balance
        p.i = i;
        n.i = -i;
        
    annotation(
        Documentation(info="<html>
            <p>This component represents an ideal electrical inductor.</p>
            <p>The inductance L is constant.</p>
        </html>"),
        Icon(
            coordinateSystem(preserveAspectRatio=true),
            graphics={
                Line(points={{-90,0},{-60,0}}),
                Line(points={{60,0},{90,0}}),
                Rectangle(
                    extent={{-60,10},{60,-10}},
                    lineColor={0,0,0},
                    fillColor={255,255,255},
                    fillPattern=FillPattern.None
                ),
                Text(
                    extent={{-60,30},{60,-30}},
                    textString="L"
                )
            }
        )
    );
    end Inductor;

end Electrical; 