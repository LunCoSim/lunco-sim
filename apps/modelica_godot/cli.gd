extends SceneTree

const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")
const EquationSystem = preload("res://apps/modelica_godot/core/equation_system.gd")
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

func simulate_model(path: String) -> int:
    print("Loading model from: ", path)
    
    # Load and parse model file
    var file = FileAccess.open(path, FileAccess.READ)
    if not file:
        print("Error: Failed to open file")
        return ERR_FILE_NOT_FOUND
        
    var content = file.get_as_text()
    var parser = MOParser.new()
    var model_data = parser.parse_text(content)
    
    if model_data.is_empty():
        print("Error: Failed to parse model")
        return ERR_PARSE_ERROR
    
    print("Model loaded successfully")
    print("Starting simulation...")
    
    # Set up simulation parameters from model annotations
    var start_time = 0.0
    var stop_time = 10.0
    var step_size = 0.01
    
    if model_data.has("annotations"):
        var annotations = model_data["annotations"]
        if annotations.has("experiment"):
            var exp = annotations["experiment"]
            start_time = float(exp.get("StartTime", start_time))
            stop_time = float(exp.get("StopTime", stop_time))
            step_size = float(exp.get("Interval", step_size))
    
    # Run simulation
    var results = _run_simulation(model_data, start_time, stop_time, step_size)
    if not results:
        print("Error: Simulation failed")
        return ERR_SCRIPT_FAILED
    
    # Output results
    _output_results(results)
    return OK

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

func _run_simulation(model_data: Dictionary, start_time: float, stop_time: float, step_size: float) -> Dictionary:
    var results = {
        "time": [],
        "variables": {
            "position": [],
            "velocity": [],
            "damper_force": []
        },
        "metadata": {
            "start_time": start_time,
            "stop_time": stop_time,
            "step_size": step_size,
            "model_name": model_data.get("name", "unknown")
        }
    }
    
    # Create equation system
    var eq_system = EquationSystem.new()
    eq_system.dt = step_size  # Set time step
    
    # Create and add components
    var components = {}  # Store components by name for easy lookup
    for component_data in model_data.get("components", []):
        var comp_type = component_data.get("type", "")
        var comp_name = component_data.get("name", "")
        
        # Create component instance
        var component = _create_component(comp_type, comp_name)
        if component == null:
            print("Error: Failed to create component of type ", comp_type)
            continue
        
        # Apply modifications (parameters)
        var modifications = component_data.get("modifications", {})
        for param_name in modifications:
            component.add_parameter(param_name, float(modifications[param_name]))
        
        # Add to equation system and store in lookup
        eq_system.add_component(component)
        components[comp_name] = component
    
    # Add connections from the model
    for equation in model_data.get("equations", []):
        if equation is Dictionary and equation.has("equation"):
            var eq = equation["equation"]
            if eq.begins_with("connect("):
                # Parse connect equation: connect(comp1.port, comp2.port)
                eq = eq.trim_prefix("connect(").trim_suffix(")")
                var parts = eq.split(",")
                if parts.size() == 2:
                    var from_parts = parts[0].strip_edges().split(".")
                    var to_parts = parts[1].strip_edges().split(".")
                    
                    if from_parts.size() == 2 and to_parts.size() == 2:
                        var from_comp = components.get(from_parts[0])
                        var to_comp = components.get(to_parts[0])
                        
                        if from_comp and to_comp:
                            # Add connection equations
                            eq_system.add_equation(from_parts[0] + "." + from_parts[1] + ".position = " + 
                                                to_parts[0] + "." + to_parts[1] + ".position", null)
                            eq_system.add_equation(from_parts[0] + "." + from_parts[1] + ".velocity = " + 
                                                to_parts[0] + "." + to_parts[1] + ".velocity", null)
                            eq_system.add_equation(from_parts[0] + "." + from_parts[1] + ".force + " + 
                                                to_parts[0] + "." + to_parts[1] + ".force = 0", null)
    
    # Add initial equations
    for init_eq in model_data.get("initial_equations", []):
        if init_eq is Dictionary and init_eq.has("equation"):
            var eq_parts = init_eq["equation"].split("=")
            if eq_parts.size() == 2:
                var var_name = eq_parts[0].strip_edges()
                var value = float(eq_parts[1].strip_edges())
                eq_system.add_initial_condition(var_name, value, null)
    
    # Initialize the system
    eq_system.initialize()
    
    # Run simulation
    var t = start_time
    while t <= stop_time:
        # Store current state
        results["time"].append(t)
        
        # Get state variables from components
        var mass_pos = 0.0
        var mass_vel = 0.0
        var damper_force = 0.0
        
        # Get values using exact component names from the model
        if components.has("mass"):
            mass_pos = components["mass"].get_variable("position")
            mass_vel = components["mass"].get_variable("velocity")
        if components.has("damper"):
            damper_force = components["damper"].get_variable("force")
        
        results["variables"]["position"].append(mass_pos)
        results["variables"]["velocity"].append(mass_vel)
        results["variables"]["damper_force"].append(damper_force)
        
        # Advance simulation
        eq_system.solve_step()
        t = eq_system.time
    
    # Add parameters to metadata
    results["metadata"]["parameters"] = {}
    if components.has("mass"):
        results["metadata"]["parameters"]["mass"] = components["mass"].get_parameter("m")
    if components.has("damper"):
        results["metadata"]["parameters"]["damping_coefficient"] = components["damper"].get_parameter("d")
    
    return results

func _output_results(results: Dictionary) -> void:
    match output_format:
        "json":
            _output_json(results)
        "csv":
            _output_csv(results)
        _:
            print("Warning: Unknown output format. Defaulting to CSV")
            _output_csv(results)

func _output_csv(results: Dictionary) -> void:
    var headers = ["time"]
    headers.append_array(results["variables"].keys())
    
    print(",".join(headers))
    for i in range(results["time"].size()):
        var row = [str(results["time"][i])]
        for var_name in results["variables"].keys():
            row.append(str(results["variables"][var_name][i]))
        print(",".join(row))

func _output_json(results: Dictionary) -> void:
    print(JSON.stringify(results, "  "))

func _print_usage() -> void:
    print("""
Modelica CLI Simulator

Usage: godot --headless --script cli.gd [options] <model_file>

Options:
    --format <format>   Output format (csv or json, default: csv)
    --help, -h         Show this help message

Example:
    godot --headless --script cli.gd --format csv model.mo
""") 