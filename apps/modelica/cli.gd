extends SceneTree

const ModelicaParser = preload("res://apps/modelica_godot/core/parser/modelica/modelica_parser.gd")
const DAESystem = preload("res://apps/modelica_godot/core/system/dae/dae_system.gd")
const DAESolver = preload("res://apps/modelica_godot/core/system/dae/dae_solver.gd")
const ModelicaComponent = preload("res://apps/modelica_godot/core/modelica/modelica_component.gd")
const ModelicaConnector = preload("res://apps/modelica_godot/core/modelica/modelica_connector.gd")

var output_format: String = "csv"  # Default output format

func _init():
    print("Initializing Modelica CLI...")
    
    # Get command line arguments
    var args = OS.get_cmdline_args()
    
    # Process arguments
    var model_path = ""
    var i = 0
    while i < args.size():
        var arg = args[i]
        if arg == "--script":
            i += 1  # Skip the script path
        elif arg == "--format":
            i += 1
            if i < args.size():
                output_format = args[i].to_lower()
        elif arg == "--help" or arg == "-h":
            _print_usage()
            quit(0)
        elif not arg.begins_with("--"):
            model_path = arg
        i += 1
    
    if model_path.is_empty():
        print("Error: No model file specified")
        _print_usage()
        quit(1)
        return
    
    # Load and simulate model
    var result = simulate_model(model_path)
    if result != OK:
        quit(1)
    else:
        quit(0)

func _print_usage() -> void:
    print("Usage: godot --script cli.gd [options] <model_file>")
    print("Options:")
    print("  --format <format>  Output format (csv, json)")
    print("  --help, -h        Show this help message")

func simulate_model(model_path: String) -> int:
    print("\nSimulating model: ", model_path)
    
    # Load model file
    var file = FileAccess.open(model_path, FileAccess.READ)
    if not file:
        push_error("Failed to open model file: " + model_path)
        return ERR_FILE_NOT_FOUND
    
    var content = file.get_as_text()
    
    # Parse model
    var parser = ModelicaParser.new()
    var ast = parser.parse(content)
    if parser.has_errors():
        for error in parser.get_errors():
            push_error(error)
        return ERR_PARSE_ERROR
    
    # Create DAE system
    var dae_system = DAESystem.new()
    var solver = DAESolver.new(dae_system)
    
    # Add equations and variables from AST
    _build_dae_system(ast, dae_system)
    
    # Initialize system
    if not solver.solve_initialization():
        push_error("Failed to initialize system")
        return ERR_CANT_CREATE
    
    # Simulate
    var t = 0.0
    var dt = 0.01
    var t_end = 10.0
    
    while t < t_end:
        if not solver.solve_continuous(dt):
            push_error("Simulation failed at t = %f" % t)
            return ERR_CANT_CREATE
        t += dt
        
        # Output results based on format
        match output_format:
            "csv":
                _output_csv(t, dae_system)
            "json":
                _output_json(t, dae_system)
    
    return OK

func _build_dae_system(ast: ASTNode, dae_system: DAESystem) -> void:
    # Add variables
    for child in ast.children:
        if child.type == ASTNode.NodeType.VARIABLE:
            var var_type = DAESystem.VariableType.ALGEBRAIC
            if child.variability == "parameter":
                var_type = DAESystem.VariableType.PARAMETER
            elif child.is_state_variable():
                var_type = DAESystem.VariableType.STATE
            dae_system.add_variable(child.value, var_type)
        elif child.is_equation():
            dae_system.add_equation(child)

func _output_csv(t: float, system: DAESystem) -> void:
    var line = "%f" % t
    for var_name in system.variables:
        line += ",%f" % system.variables[var_name].value
    print(line)

func _output_json(t: float, system: DAESystem) -> void:
    var data = {
        "time": t,
        "variables": {}
    }
    for var_name in system.variables:
        data.variables[var_name] = system.variables[var_name].value
    print(JSON.stringify(data))

func _create_component(type: String, name: String) -> ModelicaComponent:
    var component = ModelicaComponent.new(name)
    
    # Initialize component based on type
    match type:
        "Mass":
            # Add mechanical connector
            component.add_connector("port", ModelicaConnector.Type.MECHANICAL)
            
            # Add state variables and parameters
            component.add_state_variable("position", 0.0)
            component.add_state_variable("velocity", 0.0)
            component.add_variable("force", 0.0)
            component.add_parameter("m", 1.0)
            
            # Add equations
            component.add_equation("der(position) = velocity")
            component.add_equation("der(velocity) = force/m")
            component.add_equation("port.position = position")
            component.add_equation("port.velocity = velocity")
            component.add_equation("port.force = -force")  # Action-reaction principle
            
        "Damper":
            # Add mechanical connectors
            component.add_connector("port_a", ModelicaConnector.Type.MECHANICAL)
            component.add_connector("port_b", ModelicaConnector.Type.MECHANICAL)
            
            # Add parameter and variables
            component.add_parameter("d", 0.5)  # Damping coefficient
            component.add_variable("force", 0.0)
            
            # Add equations - damping force proportional to relative velocity
            component.add_equation("force = d * (port_b.velocity - port_a.velocity)")
            component.add_equation("port_a.force = force")
            component.add_equation("port_b.force = -force")  # Action-reaction principle
            
        "Fixed":
            # Add mechanical connector
            component.add_connector("port", ModelicaConnector.Type.MECHANICAL)
            
            # Add parameter for fixed position
            component.add_parameter("position", 0.0)
            
            # Add equations - fixed point doesn't move
            component.add_equation("port.position = position")
            component.add_equation("port.velocity = 0")
    
    return component 