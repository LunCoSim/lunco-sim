extends SceneTree

const Parser = preload("./core/parser.gd")
const ModelicaLexer = preload("./core/lexer.gd")
const ASTNode = preload("./core/ast_node.gd")

# Configuration
var models_path: String = "res://apps/modelica/models"
var output_format: String = "csv"
var output_file: String = ""

# Simulation settings
var simulation_start: float = 0.0
var simulation_stop: float = 10.0
var simulation_step: float = 0.01

func _init():
	print("Modelica Loader v1.0")
	
	# Process command line arguments
	var args = OS.get_cmdline_args()
	var model_name = ""
	var i = 0
	
	while i < args.size():
		var arg = args[i]
		
		if arg == "--script":
			i += 1  # Skip the script path
		elif arg == "--format" or arg == "-f":
			i += 1
			if i < args.size():
				output_format = args[i].to_lower()
		elif arg == "--output" or arg == "-o":
			i += 1
			if i < args.size():
				output_file = args[i]
		elif arg == "--start" or arg == "-s":
			i += 1
			if i < args.size():
				simulation_start = float(args[i])
		elif arg == "--stop" or arg == "-e":
			i += 1
			if i < args.size():
				simulation_stop = float(args[i])
		elif arg == "--step" or arg == "-dt":
			i += 1
			if i < args.size():
				simulation_step = float(args[i])
		elif arg == "--help" or arg == "-h":
			_print_usage()
			quit(0)
		elif not arg.begins_with("--") and not arg.begins_with("-"):
			model_name = arg
		i += 1
	
	if model_name.is_empty():
		print("Error: No model specified")
		_print_usage()
		quit(1)
	
	# Run the simulation
	var result = load_and_simulate_model(model_name)
	if result != OK:
		quit(1)
	else:
		quit(0)

func _print_usage() -> void:
	print("Usage: godot --script modelica_loader.gd [options] <model_name>")
	print("Options:")
	print("  --format, -f <format>    Output format (csv, json)")
	print("  --output, -o <file>      Output file (defaults to stdout)")
	print("  --start, -s <time>       Simulation start time (default: 0.0)")
	print("  --stop, -e <time>        Simulation end time (default: 10.0)")
	print("  --step, -dt <step>       Simulation time step (default: 0.01)")
	print("  --help, -h               Show this help message")

func load_and_simulate_model(model_name: String) -> int:
	print("\nLoading model: ", model_name)
	
	# Find the model file
	var model_file_path = find_model_file(model_name)
	if model_file_path.is_empty():
		push_error("Model not found: " + model_name)
		return ERR_FILE_NOT_FOUND
	
	print("Found model at: " + model_file_path)
	
	# Load model file
	var file = FileAccess.open(model_file_path, FileAccess.READ)
	if not file:
		push_error("Failed to open model file: " + model_file_path)
		return ERR_FILE_NOT_FOUND
	
	var content = file.get_as_text()
	file.close()
	
	# Parse model
	print("Parsing model...")
	var parser = Parser.create_modelica_parser()
	var ast = parser.parse(content)
	
	if parser._has_errors():
		print("Error parsing model:")
		for error in parser.get_errors():
			print("  " + error)
		return
	
	# Set up equations system
	print("Setting up equation system...")
	var system = setup_equation_system(ast)
	if not system:
		push_error("Failed to set up equation system")
		return ERR_CANT_CREATE
	
	# Initialize output
	var output_writer = setup_output_writer()
	if not output_writer:
		push_error("Failed to set up output writer")
		return ERR_CANT_CREATE
	
	# Run simulation
	print("Running simulation...")
	var result = simulate(system, output_writer)
	if result != OK:
		push_error("Simulation failed")
		return result
	
	print("Simulation completed successfully")
	return OK

func find_model_file(model_name: String) -> String:
	# Try direct path first
	if FileAccess.file_exists(model_name):
		return model_name
	
	# Try with .mo extension
	if not model_name.ends_with(".mo"):
		model_name += ".mo"
	
	# Try in models directory
	var model_path = models_path.path_join(model_name)
	if FileAccess.file_exists(model_path):
		return model_path
	
	# Try in Mechanical subdirectory
	model_path = models_path.path_join("Mechanical").path_join(model_name)
	if FileAccess.file_exists(model_path):
		return model_path
	
	return ""

func setup_equation_system(ast: ASTNode):
	# This is a placeholder for the actual equation system setup
	# In a real implementation, this would:
	# 1. Extract variables, parameters, and equations from the AST
	# 2. Create a DAE system
	# 3. Set up initial conditions
	
	# For now, return a simple dictionary as a mock system
	var system = {
		"variables": {},
		"parameters": {},
		"equations": [],
		"initial_conditions": {}
	}
	
	# Extract model information from AST
	for node in ast.children:
		if node.type == ASTNode.NodeType.VARIABLE:
			if node.variability == "parameter":
				system.parameters[node.name] = node.value
			else:
				system.variables[node.name] = 0.0
		elif node.type == ASTNode.NodeType.EQUATION:
			system.equations.append(node.value)
	
	return system

func setup_output_writer():
	# Create appropriate output handler based on format
	var writer = {
		"format": output_format,
		"file": null,
		"buffer": []
	}
	
	if not output_file.is_empty():
		writer.file = FileAccess.open(output_file, FileAccess.WRITE)
		if not writer.file:
			push_error("Failed to open output file: " + output_file)
			return null
	
	# Write header for CSV
	if output_format == "csv":
		var header = "time"
		# Add variable names to header
		# (In a real implementation, you would iterate through your system's variables)
		writer.buffer.append(header)
	
	return writer

func simulate(system, output_writer):
	# This is a placeholder for the actual simulation
	# In a real implementation, this would:
	# 1. Initialize the solver
	# 2. Step through time
	# 3. Solve at each step
	# 4. Output results
	
	var t = simulation_start
	while t <= simulation_stop:
		# Placeholder for solving the system at time t
		var results = {
			"time": t,
			"values": {}
		}
		
		# Output results
		write_output(output_writer, results)
		
		t += simulation_step
	
	# Close output file if needed
	if output_writer.file:
		output_writer.file.close()
	
	return OK

func write_output(writer, results):
	# Format and write results based on output format
	if writer.format == "csv":
		var line = str(results.time)
		for var_name in results.values:
			line += "," + str(results.values[var_name])
		
		if writer.file:
			writer.file.store_line(line)
		else:
			print(line)
	
	elif writer.format == "json":
		var json_text = JSON.stringify(results)
		
		if writer.file:
			writer.file.store_line(json_text)
		else:
			print(json_text) 