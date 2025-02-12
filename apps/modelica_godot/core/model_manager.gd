@tool
extends Node
class_name ModelManager

signal models_loaded
signal model_loaded(model_data: Dictionary)
signal loading_progress(progress: float, message: String)

var _parser: MOParser
var _models: Dictionary = {}  # Path -> Model data
var _model_tree: Dictionary = {}  # Package hierarchy
var _cache_file: String = "user://modelica_cache.json"
var components: Array[ModelicaComponent] = []
var equation_system: EquationSystem
var time: float = 0.0
var dt: float = 0.01  # Time step

func _init() -> void:
	print("DEBUG: ModelManager initializing")
	_models = {}
	_model_tree = {}
	
func _ready() -> void:
	_parser = MOParser.new()
	equation_system = EquationSystem.new()
	add_child(equation_system)
	# Clear cache on startup for now
	_clear_cache()
	
	# Get the absolute path to MSL directory
	var project_root = ProjectSettings.globalize_path("res://")
	var msl_path = project_root.path_join("apps/modelica_godot/MSL")
	print("DEBUG: Project root: ", project_root)
	print("DEBUG: MSL path: ", msl_path)
	
	# Start loading MSL
	if DirAccess.dir_exists_absolute(msl_path):
		print("DEBUG: MSL directory exists, starting load")
		load_msl_directory(msl_path)
	else:
		push_error("MSL directory not found at: " + msl_path)

func _clear_cache() -> void:
	if FileAccess.file_exists(_cache_file):
		DirAccess.remove_absolute(_cache_file)
		print("DEBUG: Cache cleared")

func load_msl_directory(base_path: String):
	print("DEBUG: Starting MSL load from: ", base_path)
	emit_signal("loading_progress", 0.0, "Starting MSL load...")
	
	# Try to load from cache first
	if _load_from_cache():
		print("DEBUG: Loaded from cache")
		_validate_loaded_models()  # New validation step
		emit_signal("loading_progress", 1.0, "Loaded from cache")
		emit_signal("models_loaded")
		return
	
	# Recursively find all .mo files
	var mo_files = []
	_find_mo_files(base_path, mo_files)
	print("DEBUG: Found ", mo_files.size(), " .mo files")
	
	var total_files = mo_files.size()
	if total_files == 0:
		print("DEBUG: No models found in path: ", base_path)
		emit_signal("loading_progress", 1.0, "No models found")
		emit_signal("models_loaded")
		return
		
	var processed = 0
	var successful = 0
	var failed = 0
	var batch_size = 10  # Process files in batches
	
	# First, try to load the main Modelica package
	var modelica_package = base_path.path_join("Modelica/package.mo")
	print("DEBUG: Looking for main package at: ", modelica_package)
	if FileAccess.file_exists(modelica_package):
		print("DEBUG: Found main package")
		var model_data = _parser.parse_file(modelica_package)
		if model_data.size() > 0:
			print("DEBUG: Successfully parsed main package: ", model_data)
			_models[modelica_package] = model_data
			_add_to_model_tree(modelica_package, model_data)
			successful += 1
		else:
			print("DEBUG: Failed to parse main package")
			failed += 1
	
	# Then load all other .mo files in batches
	while processed < total_files:
		var batch_end = mini(processed + batch_size, total_files)
		for i in range(processed, batch_end):
			var file_path = mo_files[i]
			if file_path == modelica_package:
				continue
				
			print("DEBUG: Processing file: ", file_path)
			var model_data = _parser.parse_file(file_path)
			if model_data.size() > 0:
				var model_type = model_data.get("type", "unknown")
				var model_name = model_data.get("name", "unnamed")
				print("DEBUG: Successfully parsed ", model_type, " ", model_name, " from ", file_path)
				_models[file_path] = model_data
				_add_to_model_tree(file_path, model_data)
				successful += 1
			else:
				print("DEBUG: Failed to parse file: ", file_path)
				failed += 1
		
		processed = batch_end
		var progress = float(processed) / total_files
		var status = "Loading models... (%d/%d, %d failed)" % [successful, total_files, failed]
		emit_signal("loading_progress", progress, status)
		# Allow the UI to update
		await get_tree().process_frame
	
	print("DEBUG: Loading complete. Successfully loaded: ", successful, " Failed: ", failed)
	print("DEBUG: Final model tree: ", _model_tree)
	
	# Save to cache
	_save_to_cache()
	
	emit_signal("models_loaded")

func _find_mo_files(path: String, results: Array):
	print("DEBUG: Searching in directory: ", path)
	var dir = DirAccess.open(path)
	if dir:
		dir.include_hidden = false
		dir.include_navigational = false
		dir.list_dir_begin()
		
		while true:
			var file_name = dir.get_next()
			if file_name == "":
				break
				
			if file_name.begins_with("."):
				continue
				
			var full_path = path.path_join(file_name)
			print("DEBUG: Found: ", full_path)
			
			if dir.current_is_dir():
				_find_mo_files(full_path, results)
			elif file_name.ends_with(".mo"):
				print("DEBUG: Adding .mo file: ", full_path)
				results.append(full_path)
		
		dir.list_dir_end()
	else:
		print("DEBUG: Failed to open directory: ", path)

func _add_to_model_tree(file_path: String, model_data: Dictionary):
	print("DEBUG: Adding to model tree: ", file_path)
	print("DEBUG: Model data: ", model_data)
	
	# Get the path relative to the MSL/Modelica directory
	var msl_base = "res://apps/modelica_godot/MSL/"
	var relative_path = file_path
	
	# Handle both absolute and relative paths
	if file_path.begins_with("/"):
		# Convert absolute path to project relative path
		var project_root = ProjectSettings.globalize_path("res://")
		if file_path.begins_with(project_root):
			relative_path = file_path.substr(project_root.length())
			print("DEBUG: Converted to relative path: ", relative_path)
	
	# Check if path is under MSL directory
	if relative_path.begins_with(msl_base) or relative_path.find("/MSL/") != -1:
		# Extract the part after MSL/
		var msl_index = relative_path.find("/MSL/")
		if msl_index != -1:
			relative_path = relative_path.substr(msl_index + 5)  # Skip "/MSL/"
		else:
			relative_path = relative_path.substr(msl_base.length())
		
		print("DEBUG: Path after MSL: ", relative_path)
		
		# Only process files under the Modelica directory
		if not relative_path.begins_with("Modelica/"):
			print("DEBUG: Skipping non-Modelica path: ", relative_path)
			return
			
		# Remove the "Modelica/" prefix
		relative_path = relative_path.substr("Modelica/".length())
		print("DEBUG: Final relative path: ", relative_path)
	else:
		print("DEBUG: Skipping file outside MSL: ", relative_path)
		return  # Skip files outside the MSL directory
	
	var path_parts = relative_path.split("/")
	if path_parts.size() == 0:
		print("DEBUG: Empty path parts")
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
				print("DEBUG: Added root Modelica package")
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
			print("DEBUG: Added model: ", model_name)

func _save_to_cache() -> void:
	# Don't save empty data
	if _models.is_empty() or _model_tree.is_empty():
		print("DEBUG: Not saving empty data to cache")
		return
		
	var cache = FileAccess.open(_cache_file, FileAccess.WRITE)
	if cache:
		var cache_data = {
			"models": _models,
			"tree": _model_tree,
			"timestamp": Time.get_unix_time_from_system()
		}
		var json_str = JSON.stringify(cache_data)
		cache.store_string(json_str)
		print("DEBUG: Saved to cache. Model count: ", _models.size())

func _load_from_cache() -> bool:
	if not FileAccess.file_exists(_cache_file):
		print("DEBUG: No cache file found")
		return false
		
	var cache = FileAccess.open(_cache_file, FileAccess.READ)
	if not cache:
		print("DEBUG: Failed to open cache file")
		return false
		
	var json = JSON.new()
	var error = json.parse(cache.get_as_text())
	if error != OK:
		print("DEBUG: Failed to parse cache JSON")
		return false
		
	var data = json.get_data()
	# Check if cache is less than a day old
	if Time.get_unix_time_from_system() - data.get("timestamp", 0) > 86400:
		print("DEBUG: Cache is too old")
		return false
		
	# Verify cache data
	if not data.has("models") or not data.has("tree"):
		print("DEBUG: Cache data is incomplete")
		return false
		
	# Check if the model tree is empty
	if data.get("tree", {}).is_empty():
		print("DEBUG: Cache contains empty model tree")
		return false
		
	_models = data.get("models", {})
	_model_tree = data.get("tree", {})
	
	print("DEBUG: Successfully loaded from cache. Model count: ", _models.size())
	print("DEBUG: Model tree from cache: ", _model_tree.keys())
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

func _validate_loaded_models():
	var total = _models.size()
	var types = {}
	for path in _models:
		var model = _models[path]
		var type = model.get("type", "unknown")
		if not types.has(type):
			types[type] = 0
		types[type] += 1
	
	print("DEBUG: Model validation:")
	print("Total models loaded: ", total)
	print("Models by type: ", types) 
