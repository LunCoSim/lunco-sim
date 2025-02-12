@tool
extends Node
class_name MOLoader

var parser: MOParser

func _ready() -> void:
	parser = MOParser.new()

# Load a Modelica component from MSL or a .mo file and create a node for it
func load_component(component_path: String) -> Node:
	print("Attempting to load Modelica component: ", component_path)
	if not parser:
		parser = MOParser.new()
	
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
		
	var model_data: Dictionary = parser.parse_file(mo_path)
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
