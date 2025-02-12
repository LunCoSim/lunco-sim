model RLCCircuit
    "RLC circuit example"
    
    // Components
    Electrical.VoltageSource source(V=12) 
        "Voltage source" 
        annotation(Placement(transformation(extent={{-80,-10},{-60,10}})));
    
    Electrical.Resistor R1(R=100) 
        "Resistor" 
        annotation(Placement(transformation(extent={{-40,-10},{-20,10}})));
    
    Electrical.Inductor L1(L=0.1) 
        "Inductor" 
        annotation(Placement(transformation(extent={{0,-10},{20,10}})));
    
    Electrical.Capacitor C1(C=1e-6) 
        "Capacitor" 
        annotation(Placement(transformation(extent={{40,-10},{60,10}})));
    
    Electrical.Ground ground 
        "Ground node" 
        annotation(Placement(transformation(extent={{-80,-40},{-60,-20}})));

equation
    // Connect components
    connect(source.p, R1.p);
    connect(R1.n, L1.p);
    connect(L1.n, C1.p);
    connect(C1.n, source.n);
    connect(source.n, ground.p);
    
annotation(
    Documentation(info="<html>
        <p>This model demonstrates a series RLC circuit.</p>
        <p>The circuit consists of:</p>
        <ul>
            <li>12V voltage source</li>
            <li>100Ω resistor</li>
            <li>0.1H inductor</li>
            <li>1µF capacitor</li>
        </ul>
    </html>"),
    experiment(
        StopTime=0.01,
        Interval=0.0001
    ),
    Icon(coordinateSystem(preserveAspectRatio=true))
);
end RLCCircuit; 