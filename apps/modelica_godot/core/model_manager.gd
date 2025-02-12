class_name ModelManager
extends Node

signal models_loaded
signal model_loaded(model_data: Dictionary)
signal loading_progress(progress: float, message: String)

var _parser: MOParser
var _models: Dictionary = {}  # Path -> Model data
var _model_tree: Dictionary = {}  # Package hierarchy
var _cache_file = "user://modelica_cache.json"
var components: Array[ModelicaComponent] = []
var equation_system: EquationSystem
var time: float = 0.0
var dt: float = 0.01  # Time step

func _init():
	_parser = MOParser.new()
	equation_system = EquationSystem.new()
	add_child(equation_system)

func load_msl_directory(base_path: String):
	emit_signal("loading_progress", 0.0, "Starting MSL load...")
	
	# Try to load from cache first
	if _load_from_cache():
		emit_signal("loading_progress", 1.0, "Loaded from cache")
		emit_signal("models_loaded")
		return
	
	# Recursively find all .mo files
	var mo_files = []
	_find_mo_files(base_path, mo_files)
	
	var total_files = mo_files.size()
	if total_files == 0:
		emit_signal("loading_progress", 1.0, "No models found")
		emit_signal("models_loaded")
		return
		
	var processed = 0
	var batch_size = 10  # Process files in batches
	
	# First, try to load the main Modelica package
	var modelica_package = base_path.path_join("Modelica/package.mo")
	if FileAccess.file_exists(modelica_package):
		var model_data = _parser.parse_file(modelica_package)
		if model_data.size() > 0:
			_models[modelica_package] = model_data
			_add_to_model_tree(modelica_package, model_data)
	
	# Then load all other .mo files in batches
	while processed < total_files:
		var batch_end = mini(processed + batch_size, total_files)
		for i in range(processed, batch_end):
			var file_path = mo_files[i]
			if file_path == modelica_package:
				continue
				
			var model_data = _parser.parse_file(file_path)
			if model_data.size() > 0:
				_models[file_path] = model_data
				_add_to_model_tree(file_path, model_data)
		
		processed = batch_end
		var progress = float(processed) / total_files
		emit_signal("loading_progress", progress, "Loading models...")
		# Allow the UI to update
		await get_tree().process_frame
	
	# Save to cache
	_save_to_cache()
	
	emit_signal("models_loaded")

func _find_mo_files(path: String, results: Array):
	var dir = DirAccess.open(path)
	if dir:
		dir.list_dir_begin()
		var file_name = dir.get_next()
		
		while file_name != "":
			var full_path = path.path_join(file_name)
			if dir.current_is_dir() and not file_name.begins_with("."):
				_find_mo_files(full_path, results)
			elif file_name.ends_with(".mo"):
				results.append(full_path)
			file_name = dir.get_next()
		
		dir.list_dir_end()

func _add_to_model_tree(file_path: String, model_data: Dictionary):
	# Get the path relative to the MSL/Modelica directory
	var msl_base = "res://apps/modelica_godot/MSL/"
	var relative_path = file_path
	if file_path.begins_with(msl_base):
		relative_path = file_path.substr(msl_base.length())
		# Only process files under the Modelica directory
		if not relative_path.begins_with("Modelica/"):
			return
		# Remove the "Modelica/" prefix
		relative_path = relative_path.substr("Modelica/".length())
	else:
		return  # Skip files outside the MSL directory
	
	var path_parts = relative_path.split("/")
	if path_parts.size() == 0:
		return
		
	var current_dict = _model_tree
	
	# Handle package.mo files specially
	if path_parts[-1] == "package.mo":
		path_parts = path_parts.slice(0, -1)  # Remove the last element
		if path_parts.size() == 0:
			# This is the root package.mo, add it directly to root
			if not current_dict.has("Modelica"):
				current_dict["Modelica"] = {
					"path": file_path,
					"type": model_data.get("type", "unknown"),
					"name": model_data.get("name", "Modelica"),
					"description": model_data.get("description", "")
				}
			return
	
	# Start from Modelica package
	if not current_dict.has("Modelica"):
		current_dict["Modelica"] = {}
	current_dict = current_dict["Modelica"]
	
	# Process intermediate directories
	for i in range(path_parts.size() - 1):
		var part = path_parts[i]
		if not current_dict.has(part):
			current_dict[part] = {}
		current_dict = current_dict[part]
	
	# Add the actual model
	if path_parts.size() > 0:
		var model_name = path_parts[-1].get_basename()
		if not model_name.is_empty():
			current_dict[model_name] = {
				"path": file_path,
				"type": model_data.get("type", "unknown"),
				"name": model_data.get("name", model_name),
				"description": model_data.get("description", "")
			}

func _save_to_cache() -> void:
	var cache = FileAccess.open(_cache_file, FileAccess.WRITE)
	if cache:
		var cache_data = {
			"models": _models,
			"tree": _model_tree,
			"timestamp": Time.get_unix_time_from_system()
		}
		cache.store_string(JSON.stringify(cache_data))

func _load_from_cache() -> bool:
	if not FileAccess.file_exists(_cache_file):
		return false
		
	var cache = FileAccess.open(_cache_file, FileAccess.READ)
	if not cache:
		return false
		
	var json = JSON.new()
	var error = json.parse(cache.get_as_text())
	if error != OK:
		return false
		
	var data = json.get_data()
	# Check if cache is less than a day old
	if Time.get_unix_time_from_system() - data.get("timestamp", 0) > 86400:
		return false
		
	_models = data.get("models", {})
	_model_tree = data.get("tree", {})
	return true

func get_model_tree() -> Dictionary:
	return _model_tree

func get_model_data(path: String) -> Dictionary:
	return _models.get(path, {})

func search_models(query: String) -> Array:
	var results = []
	query = query.to_lower()
	for path in _models:
		var model = _models[path]
		if _model_matches_search(model, query):
			results.append({
				"path": path,
				"model": model
			})
	return results

func _model_matches_search(model: Dictionary, query: String) -> bool:
	# Check name
	if model.get("name", "").to_lower().contains(query):
		return true
	
	# Check description
	if model.get("description", "").to_lower().contains(query):
		return true
	
	return false

func add_component(component: ModelicaComponent) -> void:
	components.append(component)
	add_child(component)
	
	# Add component equations to system
	for eq in component.get_equations():
		equation_system.add_equation(eq, component)

func get_component(node_name: StringName) -> ModelicaComponent:
	# Convert StringName to NodePath
	var node_path = NodePath(node_name)
	# Find component by name
	for component in components:
		if component.name == node_name:
			return component
	return null

func connect_components(from_component: ModelicaComponent, from_port: String, 
						to_component: ModelicaComponent, to_port: String) -> bool:
	# Verify connection compatibility
	var from_connector = from_component.get_connector(from_port)
	var to_connector = to_component.get_connector(to_port)
	
	if from_connector.type != to_connector.type:
		push_error("Cannot connect different connector types")
		return false
	
	# Add connection equations
	for var_name in from_connector.variables.keys():
		if _is_across_variable(var_name):
			# Across variables are equal
			equation_system.add_equation(
				"%s.%s.%s = %s.%s.%s" % [
					from_component.name, from_port, var_name,
					to_component.name, to_port, var_name
				],
				null
			)
		else:
			# Through variables sum to zero
			equation_system.add_equation(
				"%s.%s.%s + %s.%s.%s = 0" % [
					from_component.name, from_port, var_name,
					to_component.name, to_port, var_name
				],
				null
			)
	return true

func disconnect_components(from_component: ModelicaComponent, from_port: String,
						 to_component: ModelicaComponent, to_port: String) -> bool:
	# TODO: Remove connection equations
	return true

func _is_across_variable(var_name: String) -> bool:
	return var_name in ["position", "voltage", "temperature", "pressure"]

func simulate(duration: float) -> void:
	var steps = int(duration / dt)
	for i in range(steps):
		time += dt
		equation_system.solve() 
