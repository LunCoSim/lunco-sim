extends SceneTree

const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")
const PackageManager = preload("res://apps/modelica_godot/core/package_manager.gd")
const WorkspaceConfig = preload("res://apps/modelica_godot/core/workspace_config.gd")
const MOLoader = preload("res://apps/modelica_godot/core/mo_loader.gd")
const ModelManager = preload("res://apps/modelica_godot/core/model_manager.gd")

var model_manager: ModelManager
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
    
    # Initialize model manager
    print("Setting up model manager...")
    model_manager = ModelManager.new()
    var root = get_root()
    if root:
        root.add_child(model_manager)
        model_manager.initialize()
    else:
        print("Error: Could not get root node")
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
    
    # Convert relative path to absolute if needed
    var absolute_path = path
    if not path.begins_with("/"):
        absolute_path = ProjectSettings.globalize_path("res://").path_join(path)
    
    # Load model using model manager
    var model_data = model_manager.load_component(absolute_path)
    if model_data.is_empty():
        print("Error: Failed to load model")
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

func _run_simulation(model_data: Dictionary, start_time: float, stop_time: float, step_size: float) -> Dictionary:
    var results = {
        "time": [],
        "variables": {
            "position": [],
            "velocity": [],
            "spring_force": [],
            "damper_force": []
        },
        "metadata": {
            "start_time": start_time,
            "stop_time": stop_time,
            "step_size": step_size,
            "model_name": model_data.get("name", "unknown")
        }
    }
    
    # Get parameters from model
    var mass = 1.0  # Default mass in kg
    var spring_k = 10.0  # Default spring constant in N/m
    var damper_d = 0.5  # Default damping coefficient in N.s/m
    var x0 = 0.5  # Default initial position in m
    var v0 = 0.0  # Default initial velocity in m/s
    
    # Override defaults with model parameters
    for param in model_data.get("parameters", []):
        match param.get("name", ""):
            "mass":
                if param.has("value"):
                    mass = float(param["value"])
            "spring_k":
                if param.has("value"):
                    spring_k = float(param["value"])
            "damper_d":
                if param.has("value"):
                    damper_d = float(param["value"])
            "x0":
                if param.has("value"):
                    x0 = float(param["value"])
            "v0":
                if param.has("value"):
                    v0 = float(param["value"])
    
    # Initialize state variables
    var x = x0  # Position
    var v = v0  # Velocity
    var t = start_time
    
    # Simulation loop using simple Euler integration
    while t <= stop_time:
        # Calculate forces
        var spring_force = -spring_k * x
        var damper_force = -damper_d * v
        var total_force = spring_force + damper_force
        
        # Store current state
        results["time"].append(t)
        results["variables"]["position"].append(x)
        results["variables"]["velocity"].append(v)
        results["variables"]["spring_force"].append(spring_force)
        results["variables"]["damper_force"].append(damper_force)
        
        # Update state using Euler integration
        var a = total_force / mass  # Acceleration
        v += a * step_size  # Update velocity
        x += v * step_size  # Update position
        t += step_size
    
    # Add parameters to metadata
    results["metadata"]["parameters"] = {
        "mass": mass,
        "spring_constant": spring_k,
        "damping_coefficient": damper_d,
        "initial_position": x0,
        "initial_velocity": v0
    }
    
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