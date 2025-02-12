@tool
extends Node
class_name MOLoader

var _parser: MOParser

func _init():
	_parser = MOParser.new()

func load_msl(workspace_config: WorkspaceConfig) -> Array:
	var models = []
	var msl_path = workspace_config.get_workspace_path("MSL")
	
	if not DirAccess.dir_exists_absolute(msl_path):
		push_error("MSL directory not found at: " + msl_path)
		return models
	
	_find_mo_files(msl_path, models)
	return models

func load_workspace(workspace_config: WorkspaceConfig) -> Array:
	var models = []
	var models_path = workspace_config.get_workspace_path("MODELS")
	
	if not DirAccess.dir_exists_absolute(models_path):
		push_error("Models directory not found at: " + models_path)
		return models
	
	_find_mo_files(models_path, models)
	return models

func _find_mo_files(path: String, results: Array) -> void:
	var dir = DirAccess.open(path)
	if not dir:
		push_error("Failed to open directory: " + path)
		return
	
	dir.list_dir_begin()
	var file_name = dir.get_next()
	
	while file_name != "":
		if not file_name.begins_with("."):
			var full_path = path.path_join(file_name)
			
			if dir.current_is_dir():
				_find_mo_files(full_path, results)
			elif file_name.ends_with(".mo"):
				var model_data = _load_model_file(full_path)
				if not model_data.is_empty():
					model_data["path"] = full_path
					results.append(model_data)
		
		file_name = dir.get_next()
	
	dir.list_dir_end()

func _load_model_file(path: String) -> Dictionary:
	var file = FileAccess.open(path, FileAccess.READ)
	if not file:
		push_error("Failed to open file: " + path)
		return {}
	
	var content = file.get_as_text()
	return _parser.parse_file(content)

func load_model(path: String) -> ModelicaComponent:
	print("Loading model from: ", path)
	
	# Parse the model file
	var model_data = _parser.parse_file(path)
	if model_data.is_empty():
		push_error("Failed to parse model file: " + path)
		return null
	
	# Create component from parsed data
	var component = ModelicaComponent.new()
	
	# Set basic properties
	component.component_name = model_data.get("name", "")
	component.description = model_data.get("description", "")
	
	# Load parameters
	for param in model_data.get("parameters", []):
		var value = _parse_value(param.get("value", "0"))
		component.add_parameter(param.get("name", ""), value)
	
	# Load variables
	for var_data in model_data.get("variables", []):
		if var_data.get("flow", false):
			# Flow variables become connectors
			var connector_name = var_data.get("name", "")
			var connector_type = _get_connector_type(var_data.get("type", ""))
			component.add_connector(connector_name, connector_type)
		else:
			# Regular variables
			component.add_variable(var_data.get("name", ""))
	
	# Load equations
	for eq in model_data.get("equations", []):
		component.add_equation(eq)
	
	# Load annotations
	component.annotations = model_data.get("annotations", {}).duplicate()
	
	return component

func _parse_value(value_str: String) -> float:
	if value_str.is_empty():
		return 0.0
	return float(value_str)

func _get_connector_type(type_str: String) -> ModelicaConnector.Type:
	match type_str.to_lower():
		"mechanical":
			return ModelicaConnector.Type.MECHANICAL
		"electrical":
			return ModelicaConnector.Type.ELECTRICAL
		"thermal":
			return ModelicaConnector.Type.THERMAL
		"fluid":
			return ModelicaConnector.Type.FLUID
		"signal":
			return ModelicaConnector.Type.SIGNAL
		_:
			return ModelicaConnector.Type.NONE

# Load a Modelica component from MSL or a .mo file and create a node for it
func load_component(component_path: String) -> Node:
	print("Attempting to load Modelica component: ", component_path)
	if not _parser:
		_parser = MOParser.new()
	
	var mo_path: String
	if component_path.begins_with("Modelica."):
		# This is an MSL component
		mo_path = "res://MSL/" + component_path.replace(".", "/") + ".mo"
	else:
		mo_path = component_path
		
	# Skip if the file doesn't exist or is not a Modelica file
	if not FileAccess.file_exists(mo_path) or not mo_path.ends_with(".mo"):
		push_error("Invalid Modelica file path: " + mo_path)
		return null
		
	var model_data: Dictionary = _parser.parse_file(mo_path)
	if model_data.is_empty():
		push_error("Failed to parse Modelica file: " + mo_path)
		return null
		
	return _create_node_from_model(model_data)

func _create_node_from_model(model_data: Dictionary) -> Node:
	var node := Node.new()
	node.name = model_data.get("name", "UnknownModel")
	return node

# Create visual elements based on Modelica annotations
func _create_visuals(node: Node2D, annotations: Dictionary) -> void:
	if not annotations.has("Icon"):
		return
		
	var icon: Dictionary = annotations.get("Icon", {})
	if not icon.has("graphics"):
		return
		
	for graphic in icon.get("graphics", []):
		match graphic.get("type", ""):
			"Line":
				var line := Line2D.new()
				line.points = _convert_points(graphic.get("points", []))
				line.width = 2
				node.add_child(line)
			
			"Rectangle":
				var rect := ColorRect.new()
				var extent: Array = graphic.get("extent", [0, 0, 50, 50])
				rect.size = Vector2(extent[2] - extent[0], extent[3] - extent[1])
				rect.position = Vector2(extent[0], extent[1])
				if graphic.has("fillColor"):
					var fill_color: Array = graphic.get("fillColor", [0, 0, 0])
					rect.color = Color(fill_color[0]/255.0, 
									fill_color[1]/255.0, 
									fill_color[2]/255.0)
				node.add_child(rect)
			
			"Text":
				var label := Label.new()
				label.text = graphic.get("textString", "")
				var extent: Array = graphic.get("extent", [0, 0, 0, 0])
				label.position = Vector2(extent[0], extent[1])
				node.add_child(label)

# Convert Modelica point coordinates to Godot coordinates
func _convert_points(points: Array) -> PackedVector2Array:
	var result := PackedVector2Array()
	for i in range(0, points.size(), 2):
		if i + 1 < points.size():
			result.push_back(Vector2(points[i], points[i+1]))
	return result

# Component data container
class ComponentData:
	extends Node
	
	var model_type: String = ""
	var parameters: Dictionary = {}
	var variables: Dictionary = {}
	var equations: Array[String] = []
	
	func get_parameter(name: String, default: float = 0.0) -> float:
		if not parameters.has(name):
			return default
		return parameters[name].get("value", default)
	
	func get_variable(name: String, default: float = 0.0) -> float:
		if not variables.has(name):
			return default
		return variables[name].get("value", default)
	
	func set_variable(name: String, value: float) -> void:
		if variables.has(name):
			variables[name].value = value 
