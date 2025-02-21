@tool
extends Node
class_name ModelManager

const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")
const PackageManager = preload("res://apps/modelica_godot/core/package_manager.gd")
const WorkspaceConfig = preload("res://apps/modelica_godot/core/workspace_config.gd")
const MOLoader = preload("res://apps/modelica_godot/core/mo_loader.gd")
const ComponentLoader = preload("res://apps/modelica_godot/core/component_loader.gd")

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
var _component_loader: ComponentLoader

func _init() -> void:
	_models = {}
	_model_tree = {}
	_parser = MOParser.new()
	equation_system = EquationSystem.new()
	_workspace_config = WorkspaceConfig.new()
	_package_manager = PackageManager.new()
	_component_loader = ComponentLoader.new()
	add_child(_package_manager)
	add_child(_component_loader)

func _enter_tree() -> void:
	if not equation_system:
		equation_system = EquationSystem.new()
	if not equation_system.get_parent():
		add_child(equation_system)

func _ready() -> void:
	if not _workspace_config:
		push_error("WorkspaceConfig not initialized")
		return
	if not _package_manager:
		push_error("PackageManager not initialized")
		return

func initialize() -> void:
	if not _workspace_config:
		_workspace_config = WorkspaceConfig.new()
	
	if not _package_manager:
		_package_manager = PackageManager.new()
		add_child(_package_manager)
	
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

func get_component(name: String) -> Dictionary:
	return _component_loader.get_component_by_name(name)

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
	if model_data.is_empty():
		return
		
	var model_name = model_data.get("name", "")
	if model_name.is_empty():
		push_error("Model data missing name")
		return
		
	# Create a new component for the model
	var component = ModelicaComponent.new(model_name, model_data.get("description", ""))
	
	# Add parameters with proper handling
	var params = get_model_parameters(model_data)
	for param_name in params:
		add_parameter_to_component(component, param_name, params[param_name])
	
	# Add variables
	for var_data in model_data.get("variables", []):
		var var_name = var_data.get("name", "")
		var var_value = float(var_data.get("value", "0"))
		component.add_variable(var_name, var_value)
	
	# Add equations
	for equation in model_data.get("equations", []):
		component.add_equation(equation)
	
	# Add annotations
	component.annotations = model_data.get("annotations", {})
	
	# Add to model tree
	_model_tree[model_name] = component
	add_child(component)
	components.append(component)
	
	emit_signal("model_loaded", model_data)

func has_model(model_name: String) -> bool:
	return _model_tree.has(model_name)

func has_component(name: String) -> bool:
	return _component_loader.has_component(name)

func get_model_parameters(model_data: Dictionary) -> Dictionary:
	var params = {}
	for param in model_data.get("parameters", []):
		var param_name = param.get("name", "")
		if param_name.is_empty():
			continue
			
		var param_info = {
			"value": _convert_parameter_value(param),
			"type": param.get("type", "Real"),
			"description": param.get("description", ""),
			"unit": param.get("unit", ""),
			"fixed": param.get("fixed", true),
			"evaluate": param.get("evaluate", true),
			"min": param.get("min"),
			"max": param.get("max")
		}
		
		# Validate parameter value
		if not _validate_parameter(param_info):
			push_warning("Invalid parameter value for " + param_name)
			continue
			
		params[param_name] = param_info
	return params

func _convert_parameter_value(param: Dictionary) -> Variant:
	var value = param.get("value", "")
	var default = param.get("default", "")
	
	# Use default if value is empty
	if value.is_empty() and not default.is_empty():
		value = default
	
	# Convert based on type
	match param.get("type", "Real"):
		"Real":
			if value.is_empty():
				return 0.0
			return float(value)
		"Integer":
			if value.is_empty():
				return 0
			return int(value)
		"Boolean":
			if value.is_empty():
				return false
			return value.to_lower() == "true"
		"String":
			if value.is_empty():
				return ""
			return value.strip_edges().trim_prefix("\"").trim_suffix("\"")
		_:
			return value

func _validate_parameter(param_info: Dictionary) -> bool:
	var value = param_info["value"]
	var type = param_info["type"]
	
	# Type validation
	match type:
		"Real":
			if not (value is float or value is int):
				return false
		"Integer":
			if not value is int:
				return false
		"Boolean":
			if not value is bool:
				return false
		"String":
			if not value is String:
				return false
	
	# Range validation for numeric types
	if type in ["Real", "Integer"]:
		var min_val = param_info["min"]
		var max_val = param_info["max"]
		
		if min_val != null and value < min_val:
			return false
		if max_val != null and value > max_val:
			return false
	
	return true

func add_parameter_to_component(component: ModelicaComponent, param_name: String, param_info: Dictionary) -> void:
	if not param_info.has("value"):
		push_error("Parameter " + param_name + " has no value")
		return
		
	var value = param_info["value"]
	var unit = ModelicaConnector.Unit.NONE
	
	# Convert unit string to enum if present
	if param_info.has("unit") and not param_info["unit"].is_empty():
		unit = _convert_unit_string(param_info["unit"])
	
	# Add parameter to component with proper type and unit
	component.add_parameter(param_name, value, unit)
	
	# Store additional metadata if needed
	if param_info.has("min"):
		component.set_parameter_min(param_name, param_info["min"])
	if param_info.has("max"):
		component.set_parameter_max(param_name, param_info["max"])

func _convert_unit_string(unit_str: String) -> ModelicaConnector.Unit:
	match unit_str.to_lower():
		"m", "meter", "meters":
			return ModelicaConnector.Unit.METER
		"kg", "kilogram", "kilograms":
			return ModelicaConnector.Unit.KILOGRAM
		"s", "second", "seconds":
			return ModelicaConnector.Unit.SECOND
		"n", "newton", "newtons":
			return ModelicaConnector.Unit.NEWTON
		"j", "joule", "joules":
			return ModelicaConnector.Unit.JOULE
		"w", "watt", "watts":
			return ModelicaConnector.Unit.WATT
		"v", "volt", "volts":
			return ModelicaConnector.Unit.VOLT
		"a", "ampere", "amperes":
			return ModelicaConnector.Unit.AMPERE
		_:
			return ModelicaConnector.Unit.NONE

func get_experiment_settings(model_data: Dictionary) -> Dictionary:
	var settings = {}
	var annotations = model_data.get("annotations", {})
	
	if annotations.has("experiment"):
		var experiment = annotations["experiment"]
		if experiment is Dictionary:
			settings["StartTime"] = float(experiment.get("StartTime", "0"))
			settings["StopTime"] = float(experiment.get("StopTime", "1"))
			settings["Interval"] = float(experiment.get("Interval", "0.1"))
	
	return settings

func get_component_type(type_name: String, current_package: String) -> Dictionary:
	return _package_manager.resolve_type(type_name, current_package)

func load_component(path: String) -> Dictionary:
	return _component_loader.load_component_file(path)
