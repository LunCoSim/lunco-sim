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
    
    # Verify file exists
    if not FileAccess.file_exists(absolute_path):
        print("Error: Model file not found: ", absolute_path)
        return ERR_FILE_NOT_FOUND
    
    # Load model
    var file = FileAccess.open(absolute_path, FileAccess.READ)
    if not file:
        print("Error: Could not open file: ", absolute_path)
        return ERR_FILE_CANT_OPEN
        
    var content = file.get_as_text()
    file.close()
    
    var parser = MOParser.new()
    var model_data = parser.parse_text(content)
    
    if not model_data or model_data.is_empty():
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
    
    # Override defaults with model parameters if available
    if model_data.has("components"):
        for component in model_data["components"]:
            if component.has("name") and component.has("modifications"):
                match component["name"]:
                    "mass":
                        if component["modifications"].has("m"):
                            mass = float(component["modifications"]["m"])
                    "spring":
                        if component["modifications"].has("k"):
                            spring_k = float(component["modifications"]["k"])
                    "damper":
                        if component["modifications"].has("d"):
                            damper_d = float(component["modifications"]["d"])
    
    if model_data.has("parameters"):
        for param in model_data["parameters"]:
            if param.has("name") and param.has("default"):
                match param["name"]:
                    "x0":
                        x0 = float(param["default"].split(" ")[0])
                    "v0":
                        v0 = float(param["default"].split(" ")[0])
    
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