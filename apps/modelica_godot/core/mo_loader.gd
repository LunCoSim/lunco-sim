class_name MOLoader
extends Node

var parser: MOParser

func _ready():
	parser = MOParser.new()

# Load a Modelica component from a .mo file and create a node for it
func load_component(mo_path: String) -> Node:
	print("Attempting to load Modelica file: ", mo_path)
	if not parser:
		parser = MOParser.new()
		
	var model = parser.parse_file(mo_path)
	if model.is_empty():
		push_error("Failed to parse Modelica file: " + mo_path)
		return null
		
	print("Parsed model: ", model)
		
	# Create a node for this component
	var node = Node2D.new()
	node.name = model.name if model.has("name") else "Component"
	print("Created node with name: ", node.name)
	
	# Add component data
	var data = ComponentData.new()
	data.model_type = model.get("type", "")
	
	# Convert parameters array to dictionary
	var params_dict = {}
	for param in model.get("parameters", []):
		params_dict[param.name] = param
	data.parameters = params_dict
	
	# Convert variables array to dictionary if needed
	var vars_dict = {}
	for var_def in model.get("variables", []):
		vars_dict[var_def.name] = var_def
	data.variables = vars_dict
	
	data.equations = model.get("equations", [])
	node.add_child(data)
	print("Added component data: ", {
		"type": data.model_type,
		"parameters": data.parameters,
		"variables": data.variables,
		"equations": data.equations
	})
	
	# Add visual representation based on annotations
	if model.has("annotations"):
		print("Creating visuals from annotations")
		_create_visuals(node, model.annotations)
	else:
		print("No annotations found, using default visual")
		# Default visual - just a rectangle
		var rect = ColorRect.new()
		rect.size = Vector2(50, 50)
		rect.position = Vector2(-25, -25)
		node.add_child(rect)
	
	return node

# Create visual elements based on Modelica annotations
func _create_visuals(node: Node2D, annotations: Dictionary) -> void:
	if not annotations.has("Icon"):
		return
		
	var icon = annotations.Icon
	if not icon.has("graphics"):
		return
		
	for graphic in icon.graphics:
		match graphic.type:
			"Line":
				var line = Line2D.new()
				line.points = _convert_points(graphic.points)
				line.width = 2
				node.add_child(line)
			
			"Rectangle":
				var rect = ColorRect.new()
				var extent = graphic.extent
				rect.size = Vector2(extent[2] - extent[0], extent[3] - extent[1])
				rect.position = Vector2(extent[0], extent[1])
				if graphic.has("fillColor"):
					rect.color = Color(graphic.fillColor[0]/255.0, 
									graphic.fillColor[1]/255.0, 
									graphic.fillColor[2]/255.0)
				node.add_child(rect)
			
			"Text":
				var label = Label.new()
				label.text = graphic.textString
				label.position = Vector2(graphic.extent[0], graphic.extent[1])
				node.add_child(label)

# Convert Modelica point coordinates to Godot coordinates
func _convert_points(points: Array) -> PackedVector2Array:
	var result = PackedVector2Array()
	for i in range(0, points.size(), 2):
		result.push_back(Vector2(points[i], points[i+1]))
	return result

# Component data container
class ComponentData:
	extends Node
	
	var model_type: String = ""
	var parameters: Dictionary = {}
	var variables: Dictionary = {}
	var equations: Array = []
	
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
