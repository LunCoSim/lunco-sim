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
	_parser = MOParser.new()
	equation_system = EquationSystem.new()

func _enter_tree() -> void:
	print("DEBUG: ModelManager entering tree")
	if not equation_system:
		equation_system = EquationSystem.new()
	if not equation_system.get_parent():
		add_child(equation_system)

func _ready() -> void:
	print("DEBUG: ModelManager _ready")
	
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
		# Try relative path as fallback
		msl_path = "res://apps/modelica_godot/MSL"
		if DirAccess.dir_exists_absolute(msl_path):
			print("DEBUG: Found MSL at relative path, starting load")
			load_msl_directory(msl_path)
		else:
			push_error("MSL directory not found at relative path either: " + msl_path)

func load_msl_directory(path: String) -> void:
	print("DEBUG: Loading MSL directory: ", path)
	var dir = DirAccess.open(path)
	if not dir:
		push_error("Failed to open directory: " + path)
		return
		
	dir.list_dir_begin()
	var file_name = dir.get_next()
	
	while file_name != "":
		var full_path = path.path_join(file_name)
		
		if dir.current_is_dir() and not file_name.begins_with("."):
			load_msl_directory(full_path)
		elif file_name.ends_with(".mo"):
			print("DEBUG: Loading Modelica file: ", full_path)
			var model_data = _parser.parse_file(full_path)
			if not model_data.is_empty():
				_models[full_path] = model_data
				emit_signal("model_loaded", model_data)
				print("DEBUG: Loaded model: ", model_data.name)
				
		file_name = dir.get_next()
	
	dir.list_dir_end()
	emit_signal("models_loaded")

func _clear_cache() -> void:
	if FileAccess.file_exists(_cache_file):
		DirAccess.remove_absolute(_cache_file)
		print("DEBUG: Cache cleared")

func _find_mo_files(path: String, results: Array) -> void:
	print("DEBUG: Searching for .mo files in: ", path)
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
		push_error("Failed to open directory: " + path)

func _add_to_model_tree(path: String, model_data: Dictionary) -> void:
	var parts = path.split("/MSL/")[1].split("/")
	var current = _model_tree
	
	for i in range(parts.size() - 1):  # Skip the last part (filename)
		var part = parts[i]
		if not current.has(part):
			current[part] = {}
		current = current[part]
	
	# Add model data to the tree
	var filename = parts[-1].get_basename()
	if filename == "package":
		# For package.mo, add its contents at the current level
		current.merge({
			"description": model_data.get("description", ""),
			"name": model_data.get("name", ""),
			"path": path,
			"type": model_data.get("type", "")
		})
	else:
		# For other files, add them as leaf nodes
		current[filename] = {
			"description": model_data.get("description", ""),
			"name": model_data.get("name", ""),
			"path": path,
			"type": model_data.get("type", "")
		}

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

func _validate_loaded_models() -> void:
	print("Validating loaded models...")
	print("Model count: ", _models.size())
	print("Model tree size: ", _model_tree.size())
	
	# Print Modelica package contents
	if _model_tree.has("Modelica"):
		print("Modelica package contents:")
		for key in _model_tree["Modelica"].keys():
			print("- " + key)
	
	print("DEBUG: Model validation:")
	print("Total models loaded: ", _models.size())
	print("Models by type: ", JSON.stringify(_model_tree, "  "))
	print("Model tree structure: ", JSON.stringify(_model_tree, "  ")) 
