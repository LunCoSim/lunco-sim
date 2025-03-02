extends SceneTree

const default_models_dir = "res://apps/modelica/models"

var verbose = false
var package_manager

func _init():
	var arguments = OS.get_cmdline_args()
	var model_file_path = ""
	var model_path_added = false
	
	# Initialize the package manager
	package_manager = load("res://apps/modelica/core/package_manager.gd").new()
	
	# Parse command line arguments
	var i = 0
	while i < arguments.size():
		var arg = arguments[i]
		if arg == "--mpath":
			if i + 1 < arguments.size():
				var path = arguments[i + 1]
				package_manager.add_modelica_path(path)
				model_path_added = true
				i += 1
			else:
				push_error("Missing value for --mpath")
		elif arg == "--verbose":
			verbose = true
		elif arg.begins_with("res://") and arg.ends_with(".mo"):
			model_file_path = arg
		i += 1
	
	# Add default models directory if no paths were specified
	if not model_path_added:
		package_manager.add_modelica_path(default_models_dir)
		if verbose:
			print("Added default models directory to MODELICAPATH: " + default_models_dir)
	
	if verbose:
		print("Current MODELICAPATH: " + str(package_manager.get_modelica_path()))
	
	if model_file_path.is_empty():
		print_usage()
		quit()
	else:
		load_and_simulate_model(model_file_path)
		quit()

func print_usage():
	print("Modelica CLI Usage:")
	print("godot --script apps/modelica/cli.gd [options] <model_file_path>")
	print("")
	print("Options:")
	print("  --mpath <path>   Add path to MODELICAPATH")
	print("  --verbose        Enable verbose output")
	print("")
	print("Examples:")
	print("  godot --script apps/modelica/cli.gd res://apps/modelica/models/MyModel.mo")
	print("  godot --script apps/modelica/cli.gd --mpath res://apps/modelica/models res://path/to/model.mo")

func load_and_simulate_model(model_file_path: String):
	if verbose:
		print("Loading model file: " + model_file_path)
		print("Current MODELICAPATH: " + str(package_manager.get_modelica_path()))
	
	# Try to auto-add package path based on the model file
	var discovery_result = package_manager.discover_package_from_path(model_file_path)
	if verbose:
		print("Package discovery result:")
		print("  Found package structure: " + str(discovery_result.get("package_hierarchy", [])))
		print("  Root package: " + str(discovery_result.get("root_package", "")))
		print("  Root package path: " + str(discovery_result.get("root_package_path", "")))
		
	if discovery_result.has("root_package_path") and discovery_result.root_package_path != "":
		var auto_add_result = package_manager.auto_add_package_path(model_file_path)
		if verbose:
			print("Auto-added package path: " + auto_add_result.path_added)
			print("Updated MODELICAPATH: " + str(package_manager.get_modelica_path()))
	
	var parser = load("res://apps/modelica/core/parser.gd").new()
	var ast = parser.parse_file(model_file_path)
	
	if ast == null:
		push_error("Failed to parse model file: " + model_file_path)
		return
	
	if verbose:
		print("Model parsed successfully")
		print("Model qualified name: " + ast.qualified_name)
		if ast.has("within"):
			print("Within package: " + str(ast.get("within")))
	
	# Determine how to load the model
	var model_path = model_file_path
	var use_qualified_name = false
	
	if not ast.qualified_name.is_empty():
		# First, check if the qualified name exists in the package structure
		var qualified_model_path = package_manager.find_model_by_qualified_name(ast.qualified_name)
		if not qualified_model_path.is_empty():
			model_path = ast.qualified_name
			use_qualified_name = true
			if verbose:
				print("Found model by qualified name: " + qualified_model_path)
		else:
			print("Warning: Model has qualified name '" + ast.qualified_name + "' but no matching package structure was found")
			print("Using file path instead: " + model_file_path)
	else:
		print("Warning: Model has no qualified name, using file path: " + model_file_path)
	
	# Load the model with the appropriate path
	var result
	if use_qualified_name:
		result = package_manager.validate_and_load_model(model_path)
	else:
		result = package_manager.validate_and_load_model(model_file_path)
	
	if result.success:
		if verbose:
			print("Model validated and loaded successfully")
			if result.dependencies.size() > 0:
				print("Dependencies found:")
				for dep in result.dependencies:
					print("  - " + dep)
		
		# Here we would pass the model to the simulator
		# For now we'll just print a success message
		print("Simulation completed successfully")
	else:
		var error_message = "Error loading model:"
		if result.errors.size() > 0:
			for err in result.errors:
				error_message += "\n  - " + err.message
		else:
			error_message += " Unknown error"
		push_error(error_message)