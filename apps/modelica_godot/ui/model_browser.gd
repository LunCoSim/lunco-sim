@tool
extends Control

const ModelicaParser = preload("res://apps/modelica_godot/core/parser/modelica_parser.gd")
const DAESystem = preload("res://apps/modelica_godot/core/system/dae/dae_system.gd")

@onready var tree: Tree = $Tree
@onready var file_dialog: FileDialog = $FileDialog

signal model_selected(model_name: String, ast: Dictionary)

var parser: ModelicaParser
var current_ast: Dictionary

func _ready() -> void:
	parser = ModelicaParser.new()
	tree.item_selected.connect(_on_item_selected)

func load_model(file_path: String) -> void:
	var file = FileAccess.open(file_path, FileAccess.READ)
	if not file:
		push_error("Failed to open file: " + file_path)
		return
		
	var content = file.get_as_text()
	file.close()
	
	var result = parser.parse(content)
	if result.error:
		push_error("Failed to parse model: " + result.error)
		return
		
	current_ast = result.ast
	_populate_tree(current_ast)

func _populate_tree(ast: Dictionary) -> void:
	tree.clear()
	var root = tree.create_item()
	root.set_text(0, "Model")
	
	# Add classes
	if ast.has("classes"):
		for class_name in ast.classes:
			var class_item = tree.create_item(root)
			class_item.set_text(0, class_name)
			var class_data = ast.classes[class_name]
			
			# Add components
			if class_data.has("components"):
				var components_item = tree.create_item(class_item)
				components_item.set_text(0, "Components")
				for component in class_data.components:
					var comp_item = tree.create_item(components_item)
					comp_item.set_text(0, component.name + ": " + component.type)
			
			# Add equations
			if class_data.has("equations"):
				var equations_item = tree.create_item(class_item)
				equations_item.set_text(0, "Equations")
				for equation in class_data.equations:
					var eq_item = tree.create_item(equations_item)
					eq_item.set_text(0, _format_equation(equation))

func _format_equation(equation: Dictionary) -> String:
	# Simple equation formatting - can be expanded based on equation types
	if equation.has("left") and equation.has("right"):
		return str(equation.left) + " = " + str(equation.right)
	return str(equation)

func _on_item_selected() -> void:
	var selected = tree.get_selected()
	if selected and selected.get_parent() == tree.get_root():
		var model_name = selected.get_text(0)
		if current_ast.classes.has(model_name):
			emit_signal("model_selected", model_name, current_ast.classes[model_name])

func _on_open_button_pressed() -> void:
	file_dialog.popup_centered() 
