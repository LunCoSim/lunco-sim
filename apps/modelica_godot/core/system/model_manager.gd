@tool
extends Node
class_name ModelManager

const PackageLoader = preload("res://apps/modelica_godot/core/loader/package_loader.gd")
const ComponentLoader = preload("res://apps/modelica_godot/core/loader/component_loader.gd")

signal models_loaded_changed
signal loading_progress(progress: float, message: String)
signal model_loaded(model_data: Dictionary)

var _models: Dictionary = {}  # Path -> Model data
var _model_tree: Dictionary = {}  # Package hierarchy
var _cache_file: String = "user://modelica_cache.json"
var components: Dictionary = {}  # name -> ModelicaComponent
var _package_loader: PackageLoader
var _component_loader: ComponentLoader
var _equation_system: EquationSystem
var time: float = 0.0
var dt: float = 0.01  # Time step

func _init() -> void:
	_models = {}
	_model_tree = {}
	_package_loader = PackageLoader.new()
	_component_loader = ComponentLoader.new()
	_equation_system = EquationSystem.new()

func _ready() -> void:
	if not _package_loader:
		push_error("PackageLoader not initialized")
		return
	
	add_child(_package_loader)
	add_child(_component_loader)
	
	# Connect signals
	_package_loader.package_loaded.connect(_on_package_loaded)
	_package_loader.package_loading_error.connect(_on_package_loading_error)
	_component_loader.component_loaded.connect(_on_component_loaded)
	_component_loader.component_loading_error.connect(_on_component_loading_error)

func initialize() -> void:
	if not _package_loader:
		_package_loader = PackageLoader.new()
		add_child(_package_loader)
	
	# Load initial models
	load_models()

func set_msl_path(path: String) -> void:
	_package_loader.set_msl_path(path)

func load_models() -> void:
	# Clear existing models
	_model_tree.clear()
	
	# Load MSL if available
	if _package_loader.has_msl():
		emit_signal("loading_progress", 0.0, "Loading MSL...")
		_package_loader.load_msl()
		emit_signal("loading_progress", 0.5, "MSL loaded")
	
	# Load workspace models
	emit_signal("loading_progress", 0.5, "Loading workspace models...")
	for package in _package_loader.get_loaded_packages():
		var components = _package_loader.get_package_components(package)
		for component_name in components:
			var component_data = components[component_name]
			_add_model_to_tree(component_data.data)
	
	emit_signal("loading_progress", 1.0, "All models loaded")
	emit_signal("models_loaded_changed")

func _add_model_to_tree(model_data: Dictionary) -> void:
	var path = model_data.get("path", "")
	if path.is_empty():
		return
		
	# Extract package hierarchy from model name
	var model_name = model_data.get("name", "")
	var parts = model_name.split(".")
	var current = _model_tree
	
	# Build the tree structure
	for i in range(parts.size() - 1):
		var part = parts[i]
		if not current.has(part):
			current[part] = {
				"name": part,
				"type": "package",
				"children": {}
			}
		current = current.get(part).get("children")
	
	# Add model data to the tree
	var component_name = parts[-1]
	current[component_name] = {
		"description": model_data.get("description", ""),
		"name": model_name,
		"path": path,
		"type": model_data.get("type", "unknown"),
		"children": {}
	}
	
	# Store in flat model dictionary too
	_models[path] = model_data

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
	var name = component.get_declaration(component.declarations.keys()[0]).name
	components[name] = component

func get_component(name: String) -> ModelicaComponent:
	return components.get(name)

func connect_components(from_component: String, from_port: String, 
						to_component: String, to_port: String) -> bool:
	var from_comp = get_component(from_component)
	var to_comp = get_component(to_component)
	
	if not from_comp or not to_comp:
		push_error("Component not found")
		return false
	
	var from_connector = from_comp.get_connector(from_port)
	var to_connector = to_comp.get_connector(to_port)
	
	if not from_connector or not to_connector:
		push_error("Connector not found")
		return false
	
	if from_connector.type != to_connector.type:
		push_error("Cannot connect different connector types")
		return false
	
	# Add connection equations
	for var_name in from_connector.variables.keys():
		if from_connector.get_variable(var_name).is_flow_variable():
			# Through variables sum to zero
			_equation_system.add_equation(
				"%s.%s.%s + %s.%s.%s = 0" % [
					from_component, from_port, var_name,
					to_component, to_port, var_name
				]
			)
		else:
			# Across variables are equal
			_equation_system.add_equation(
				"%s.%s.%s = %s.%s.%s" % [
					from_component, from_port, var_name,
					to_component, to_port, var_name
				]
			)
	
	# Connect the connectors
	from_connector.connect_to(to_connector)
	return true

func disconnect_components(from_component: ModelicaComponent, from_port: String,
						 to_component: ModelicaComponent, to_port: String) -> bool:
	# TODO: Remove connection equations
	return true

func _is_flow_variable(connector_name: String, variable_name: String) -> bool:
	return _component_loader.is_flow_variable(connector_name, variable_name)

func simulate(duration: float) -> void:
	var steps = int(duration / dt)
	for i in range(steps):
		time += dt
		_equation_system.solve() 

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
func _on_package_loaded(package_data: Dictionary) -> void:
	_models[package_data.path] = package_data
	_update_model_tree()
	emit_signal("models_loaded_changed")
	emit_signal("model_loaded", package_data)

func _on_package_loading_error(error: String) -> void:
	push_error("Package loading error: " + error)

func _on_component_loaded(component_data: Dictionary) -> void:
	_models[component_data.path] = component_data
	_update_model_tree()
	emit_signal("models_loaded_changed")
	emit_signal("model_loaded", component_data)

func _on_component_loading_error(error: String) -> void:
	push_error("Component loading error: " + error)

# Public API methods for package management
func has_package(package_name: String) -> bool:
	return _package_loader.has_package(package_name)

func get_package_metadata(package_name: String) -> Dictionary:
	return _package_loader.get_package_metadata(package_name)

func get_loaded_packages() -> Array:
	return _package_loader.get_loaded_packages()

# Model Loading Methods
func load_models_from_directory(path: String) -> void:
	emit_signal("loading_progress", 0.0, "Starting model loading...")
	
	# First load the package structure
	if not _package_loader.load_package(path):
		emit_signal("loading_progress", 1.0, "Failed to load package")
		return
	
	emit_signal("loading_progress", 1.0, "Models loaded")
	emit_signal("models_loaded_changed")

func get_component_type(type_name: String, current_package: String) -> Dictionary:
	return _package_loader.resolve_type(type_name, current_package)

func load_component(path: String) -> Dictionary:
	return _component_loader.load_component_file(path)

func _update_model_tree() -> void:
	_model_tree.clear()
	for path in _models:
		var model = _models[path]
		var parts = path.split("/")
		var current = _model_tree
		
		for i in range(parts.size()):
			var part = parts[i]
			if not current.has(part):
				current[part] = {}
			current = current[part]
			
			if i == parts.size() - 1:
				current["__data"] = model

func get_model(path: String) -> Dictionary:
	return _models.get(path, {})

func get_equation_system() -> EquationSystem:
	return _equation_system

func _to_string() -> String:
	var result = "ModelManager:\n"
	result += "  Models:\n"
	for path in _models:
		result += "    %s\n" % path
	result += "  Equation System:\n"
	result += _equation_system._to_string()
	return result
