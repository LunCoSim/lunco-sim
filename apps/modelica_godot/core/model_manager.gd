@tool
extends Node
class_name ModelManager

const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")
const PackageManager = preload("res://apps/modelica_godot/core/package_manager.gd")
const WorkspaceConfig = preload("res://apps/modelica_godot/core/workspace_config.gd")
const MOLoader = preload("res://apps/modelica_godot/core/mo_loader.gd")

signal models_loaded_changed
signal loading_progress(progress: float, message: String)
signal model_loaded(model_data: Dictionary)

var _parser: MOParser
var _models: Dictionary = {}  # Path -> Model data
var _model_tree: Dictionary = {}  # Package hierarchy
var _package_tree: Dictionary = {}
var _cache_file: String = "user://modelica_cache.json"
var components: Array[ModelicaComponent] = []
var equation_system: EquationSystem
var time: float = 0.0
var dt: float = 0.01  # Time step
var _package_manager: PackageManager
var _workspace_config: WorkspaceConfig

func _init() -> void:
	_models = {}
	_model_tree = {}
	_parser = MOParser.new()
	equation_system = EquationSystem.new()
	_workspace_config = WorkspaceConfig.new()

func _enter_tree() -> void:
	if not equation_system:
		equation_system = EquationSystem.new()
	if not equation_system.get_parent():
		add_child(equation_system)

func _ready() -> void:
	if not _workspace_config:
		push_error("WorkspaceConfig not initialized")
		return

func initialize() -> void:
	if not _workspace_config:
		_workspace_config = WorkspaceConfig.new()
	
	# Load initial models
	load_models()

func load_models() -> void:
	# Clear existing models
	_model_tree.clear()
	_package_tree.clear()
	
	# Load models from workspace
	var loader = MOLoader.new()
	var parser = MOParser.new()
	
	# Load MSL if available
	if _workspace_config.has_msl():
		emit_signal("loading_progress", 0.0, "Loading MSL...")
		var msl_models = loader.load_msl(_workspace_config)
		for model in msl_models:
			_add_model_to_tree(model)
		emit_signal("loading_progress", 0.5, "MSL loaded")
	
	# Load workspace models
	emit_signal("loading_progress", 0.5, "Loading workspace models...")
	var workspace_models = loader.load_workspace(_workspace_config)
	for model in workspace_models:
		_add_model_to_tree(model)
	
	emit_signal("loading_progress", 1.0, "All models loaded")
	emit_signal("models_loaded_changed")

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
	# Extract the relative path from MSL directory
	var msl_index = path.find("/MSL/")
	if msl_index == -1:
		# Try alternative path format
		msl_index = path.find("MSL/")
		if msl_index == -1:
			push_error("Invalid model path: " + path)
			return
	
	var relative_path = path.substr(msl_index + 4)  # Skip "MSL/"
	var parts = relative_path.split("/")
	var current = _model_tree
	
	# Build the tree structure
	for i in range(parts.size() - 1):  # Skip the last part (filename)
		var part = parts[i]
		if not current.has(part):
			current[part] = {
				"name": part,
				"type": "package",
				"children": {}
			}
		current = current.get(part).get("children")
	
	# Add model data to the tree
	var filename = parts[-1].get_basename()
	if filename == "package":
		# For package.mo, merge with parent node
		current.merge({
			"description": model_data.get("description", ""),
			"name": model_data.get("name", ""),
			"path": path,
			"type": model_data.get("type", "")
		})
	else:
		# For other files, add as a child
		current[filename] = {
			"description": model_data.get("description", ""),
			"name": model_data.get("name", filename),
			"path": path,
			"type": model_data.get("type", "unknown"),
			"children": {}
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
	
	# Validate model tree structure
	var has_modelica = _model_tree.has("Modelica")
	print("Has Modelica package: ", has_modelica)
	
	if has_modelica:
		var modelica = _model_tree["Modelica"]
		print("Modelica package type: ", modelica.get("type", "unknown"))
		print("Modelica subpackages: ", modelica.get("children", {}).keys())
	
	# Check for common packages
	var packages = ["Blocks", "Electrical", "Mechanics", "Thermal"]
	for pkg in packages:
		var has_pkg = _model_tree.has("Modelica") and _model_tree["Modelica"].get("children", {}).has(pkg)
		print("Has ", pkg, " package: ", has_pkg)

# Signal Handlers
func _on_package_loaded(package_name: String) -> void:
	emit_signal("loading_progress", 0.75, "Loaded package: " + package_name)

func _on_package_loading_error(package_name: String, error: String) -> void:
	push_error("Package loading error - " + package_name + ": " + error)
	emit_signal("loading_progress", 1.0, "Error loading package: " + package_name)

# Public API methods for package management
func has_package(package_name: String) -> bool:
	return _package_manager.has_package(package_name)

func get_package_metadata(package_name: String) -> Dictionary:
	return _package_manager.get_package_metadata(package_name)

func get_loaded_packages() -> Array:
	return _package_manager.get_loaded_packages()

# Model Loading Methods
func load_models_from_directory(path: String) -> void:
	emit_signal("loading_progress", 0.0, "Starting model loading...")
	
	# First load the package structure
	if not _package_manager.load_package(path):
		emit_signal("loading_progress", 1.0, "Failed to load package")
		return
	
	emit_signal("loading_progress", 0.5, "Loading models...")
	
	# Load actual models
	_load_models_recursive(path)
	
	emit_signal("loading_progress", 1.0, "Models loaded")
	emit_signal("models_loaded_changed")

func _load_models_recursive(path: String) -> void:
	var dir = DirAccess.open(path)
	if not dir:
		push_error("Could not open directory: " + path)
		return
	
	dir.list_dir_begin()
	var file_name = dir.get_next()
	
	while file_name != "":
		if not file_name.begins_with("."):
			var full_path = path + "/" + file_name
			
			if dir.current_is_dir():
				_load_models_recursive(full_path)
			elif file_name.ends_with(".mo") and file_name != "package.mo":
				_load_model_file(full_path)
		
		file_name = dir.get_next()
	
	dir.list_dir_end()

func _load_model_file(path: String) -> void:
	var file = FileAccess.open(path, FileAccess.READ)
	if not file:
		push_error("Could not open file: " + path)
		return
	
	var content = file.get_as_text()
	_parse_model_file(content, path)

func _parse_model_file(content: String, path: String) -> void:
	# Basic model parsing
	var model_data = {
		"path": path,
		"name": path.get_file().get_basename(),
		"type": "model",
		"content": content
	}
	
	# Add to model tree
	var tree_path = path.trim_prefix(ProjectSettings.get_setting("modelica/root_path", ""))
	_add_to_model_tree(tree_path, model_data)

func _add_model_to_tree(model_data: Dictionary) -> void:
	var path = model_data.get("path", "")
	if path.is_empty():
		push_error("Invalid model path")
		return
	
	_add_to_model_tree(path, model_data)
