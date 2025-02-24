@tool
extends Control

const ModelicaParser = preload("res://apps/modelica_godot/core/parser/modelica/modelica_parser.gd")
const DAESystem = preload("res://apps/modelica_godot/core/system/dae/dae_system.gd")
const DAESolver = preload("res://apps/modelica_godot/core/system/dae/dae_solver.gd")

var simulation_running: bool = false
var has_unsaved_changes: bool = false
var current_file: String = ""
var component_count: int = 0

@onready var graph_edit: GraphEdit = $GraphEdit
@onready var status_label: Label = $StatusBar/StatusLabel
@onready var model_browser_window: Window = $ModelBrowserWindow
@onready var simulation_view: Control = $SimulationView

func _ready() -> void:
	# Initialize UI
	graph_edit.add_valid_connection_type(0, 0)  # Allow connecting ports
	graph_edit.connection_request.connect(_on_connection_request)
	graph_edit.disconnection_request.connect(_on_disconnection_request)
	
	# Initialize simulation view
	simulation_view.initialize()
	
	# Set up file menu
	var file_menu = $MenuBar/FileMenu
	file_menu.id_pressed.connect(_on_file_menu_item_selected)
	
	# Set up simulation menu
	var sim_menu = $MenuBar/SimulationMenu
	sim_menu.id_pressed.connect(_on_simulation_menu_item_selected)
	
	status_label.text = "Ready"

func _on_file_menu_item_selected(id: int) -> void:
	match id:
		0:  # New System
			_new_system()
		1:  # Import Modelica
			_import_modelica()
		2:  # Export Modelica
			_export_modelica()
		3:  # Save Workspace
			_save_workspace()
		4:  # Load Workspace
			_load_workspace()

func _new_system() -> void:
	# Clear all nodes and connections
	for node in graph_edit.get_children():
		if node is GraphNode:
			node.queue_free()
	component_count = 0
	current_file = ""
	has_unsaved_changes = false
	simulation_view.reset()
	status_label.text = "Created new system"

func _import_modelica() -> void:
	var file_dialog = FileDialog.new()
	file_dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILE
	file_dialog.access = FileDialog.ACCESS_FILESYSTEM
	file_dialog.filters = ["*.mo ; Modelica Files"]
	file_dialog.title = "Import Modelica Model"
	add_child(file_dialog)
	file_dialog.popup_centered(Vector2(800, 600))
	
	file_dialog.file_selected.connect(
		func(path):
			_import_modelica_file(path)
			file_dialog.queue_free()
	)

func _export_modelica() -> void:
	var file_dialog = FileDialog.new()
	file_dialog.file_mode = FileDialog.FILE_MODE_SAVE_FILE
	file_dialog.access = FileDialog.ACCESS_FILESYSTEM
	file_dialog.filters = ["*.mo ; Modelica Files"]
	file_dialog.title = "Export Modelica Model"
	add_child(file_dialog)
	file_dialog.popup_centered(Vector2(800, 600))
	
	file_dialog.file_selected.connect(
		func(path):
			_export_modelica_file(path)
			file_dialog.queue_free()
	)

func _save_workspace() -> void:
	var file_dialog = FileDialog.new()
	file_dialog.file_mode = FileDialog.FILE_MODE_SAVE_FILE
	file_dialog.access = FileDialog.ACCESS_FILESYSTEM
	file_dialog.filters = ["*.json ; JSON Files"]
	file_dialog.title = "Save Workspace"
	add_child(file_dialog)
	file_dialog.popup_centered(Vector2(800, 600))
	
	file_dialog.file_selected.connect(
		func(path):
			_save_workspace_file(path)
			file_dialog.queue_free()
	)

func _load_workspace() -> void:
	var file_dialog = FileDialog.new()
	file_dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILE
	file_dialog.access = FileDialog.ACCESS_FILESYSTEM
	file_dialog.filters = ["*.json ; JSON Files"]
	file_dialog.title = "Load Workspace"
	add_child(file_dialog)
	file_dialog.popup_centered(Vector2(800, 600))
	
	file_dialog.file_selected.connect(
		func(path):
			_load_workspace_file(path)
			file_dialog.queue_free()
	)

func _on_simulation_menu_item_selected(id: int) -> void:
	match id:
		0:  # Start/Stop
			if simulation_running:
				_stop_simulation()
			else:
				_start_simulation()
		1:  # Reset
			_reset_simulation()

func _start_simulation() -> void:
	if simulation_running:
		return
	
	# Create DAE system from current graph
	var dae_system = _create_dae_system()
	if not dae_system:
		status_label.text = "Failed to create simulation system"
		return
	
	# Initialize solver
	var solver = DAESolver.new(dae_system)
	if not solver.solve_initialization():
		status_label.text = "Failed to initialize simulation"
		return
	
	simulation_view.set_system(dae_system, solver)
	simulation_running = true
	status_label.text = "Simulation running"

func _stop_simulation() -> void:
	if not simulation_running:
		return
	
	simulation_running = false
	status_label.text = "Simulation stopped"

func _reset_simulation() -> void:
	simulation_view.reset()
	simulation_running = false
	status_label.text = "Simulation reset"

func _create_dae_system() -> DAESystem:
	var system = DAESystem.new()
	
	# Add variables and equations from components
	for node in graph_edit.get_children():
		if not (node is GraphNode):
			continue
		
		var component = node.get_component()
		if not component:
			continue
		
		# Add variables
		for var_name in component.variables:
			var var_obj = component.variables[var_name]
			var type = DAESystem.VariableType.ALGEBRAIC
			if var_obj.is_parameter():
				type = DAESystem.VariableType.PARAMETER
			elif var_obj.is_state():
				type = DAESystem.VariableType.STATE
			system.add_variable(node.name + "." + var_name, type)
		
		# Add equations
		for equation in component.equations:
			system.add_equation(equation)
	
	# Add connection equations
	for connection in graph_edit.get_connection_list():
		var from_node = graph_edit.get_node(connection.from)
		var to_node = graph_edit.get_node(connection.to)
		
		if not (from_node and to_node):
			continue
		
		var from_port = from_node.get_port(connection.from_port)
		var to_port = to_node.get_port(connection.to_port)
		
		if not (from_port and to_port):
			continue
		
		# Add connection equations based on port type
		_add_connection_equations(system, 
			from_node.name + "." + from_port.name,
			to_node.name + "." + to_port.name)
	
	return system

func _add_connection_equations(system: DAESystem, from_port: String, to_port: String) -> void:
	# Add appropriate equations based on port type
	# For mechanical connections:
	system.add_equation(_create_equation_node(from_port + ".position = " + to_port + ".position"))
	system.add_equation(_create_equation_node(from_port + ".velocity = " + to_port + ".velocity"))
	system.add_equation(_create_equation_node(from_port + ".force + " + to_port + ".force = 0"))

func _create_equation_node(equation_str: String) -> ASTNode:
	var parser = ModelicaParser.new()
	return parser.parse(equation_str)

func _import_modelica_file(path: String) -> void:
	var file = FileAccess.open(path, FileAccess.READ)
	if not file:
		status_label.text = "Failed to open file: " + path
		return
	
	var content = file.get_as_text()
	
	# Parse model
	var parser = ModelicaParser.new()
	var ast = parser.parse(content)
	if parser.has_errors():
		status_label.text = "Failed to parse model"
		for error in parser.get_errors():
			push_error(error)
		return
	
	# Create components from AST
	_create_model_from_ast(ast)
	
	current_file = path
	has_unsaved_changes = false
	status_label.text = "Imported model: " + path.get_file()

func _create_model_from_ast(ast: ASTNode) -> void:
	# Clear current system
	_new_system()
	
	# Create components
	for child in ast.children:
		if child.type == ASTNode.NodeType.COMPONENT:
			_create_component(child.get_metadata("type"), child.value)
	
	# Add connections
	for child in ast.children:
		if child.type == ASTNode.NodeType.CONNECT_EQUATION:
			var from_ref = child.get_metadata("from").split(".")
			var to_ref = child.get_metadata("to").split(".")
			if from_ref.size() == 2 and to_ref.size() == 2:
				_create_connection(from_ref[0], from_ref[1], to_ref[0], to_ref[1])

func _create_component(type: String, name: String) -> void:
	var node = GraphNode.new()
	node.name = name
	node.title = type
	node.position_offset = Vector2(100, 100) * (component_count + 1)
	node.size = Vector2(200, 100)
	node.set_script(load("res://apps/modelica_godot/ui/component_node.gd"))
	
	graph_edit.add_child(node)
	node.setup(type)
	component_count += 1

func _create_connection(from_comp: String, from_port: String, to_comp: String, to_port: String) -> void:
	var from_node = graph_edit.get_node_or_null(from_comp)
	var to_node = graph_edit.get_node_or_null(to_comp)
	
	if not (from_node and to_node):
		return
	
	var from_port_id = from_node.get_port_id(from_port)
	var to_port_id = to_node.get_port_id(to_port)
	
	if from_port_id >= 0 and to_port_id >= 0:
		graph_edit.connect_node(from_comp, from_port_id, to_comp, to_port_id)

func _export_modelica_file(path: String) -> void:
	# Create AST from current system
	var ast = _create_ast_from_system()
	
	# Format as Modelica code
	var code = ast.format_modelica()
	
	# Save to file
	var file = FileAccess.open(path, FileAccess.WRITE)
	if not file:
		status_label.text = "Failed to save file: " + path
		return
	
	file.store_string(code)
	current_file = path
	has_unsaved_changes = false
	status_label.text = "Exported model to: " + path.get_file()

func _create_ast_from_system() -> ASTNode:
	var ast = ASTNode.new(ASTNode.NodeType.MODEL, "Model")
	
	# Add components
	for node in graph_edit.get_children():
		if not (node is GraphNode):
			continue
		
		var component = node.get_component()
		if not component:
			continue
		
		var comp_node = ASTNode.new(ASTNode.NodeType.COMPONENT, node.name)
		comp_node.add_metadata("type", node.title)
		ast.add_child(comp_node)
	
	# Add connections
	for connection in graph_edit.get_connection_list():
		var from_node = graph_edit.get_node(connection.from)
		var to_node = graph_edit.get_node(connection.to)
		
		if not (from_node and to_node):
			continue
		
		var from_port = from_node.get_port(connection.from_port)
		var to_port = to_node.get_port(connection.to_port)
		
		if not (from_port and to_port):
			continue
		
		var connect_node = ASTNode.new(ASTNode.NodeType.CONNECT_EQUATION)
		connect_node.add_metadata("from", connection.from + "." + from_port.name)
		connect_node.add_metadata("to", connection.to + "." + to_port.name)
		ast.add_child(connect_node)
	
	return ast

func _save_workspace_file(path: String) -> void:
	var data = {
		"components": [],
		"connections": graph_edit.get_connection_list()
	}
	
	# Save component data
	for node in graph_edit.get_children():
		if not (node is GraphNode):
			continue
		
		var component = node.get_component()
		if not component:
			continue
		
		data.components.append({
			"name": node.name,
			"type": node.title,
			"position": {
				"x": node.position_offset.x,
				"y": node.position_offset.y
			},
			"parameters": component.parameters
		})
	
	# Save to file
	var file = FileAccess.open(path, FileAccess.WRITE)
	if not file:
		status_label.text = "Failed to save workspace: " + path
		return
	
	file.store_string(JSON.stringify(data, "  "))
	status_label.text = "Saved workspace to: " + path.get_file()

func _load_workspace_file(path: String) -> void:
	var file = FileAccess.open(path, FileAccess.READ)
	if not file:
		status_label.text = "Failed to load workspace: " + path
		return
	
	var json = JSON.new()
	var error = json.parse(file.get_as_text())
	if error != OK:
		status_label.text = "Failed to parse workspace file"
		return
	
	var data = json.get_data()
	
	# Clear current system
	_new_system()
	
	# Create components
	for comp_data in data.components:
		_create_component(comp_data.type, comp_data.name)
		var node = graph_edit.get_node(comp_data.name)
		if node:
			node.position_offset = Vector2(comp_data.position.x, comp_data.position.y)
			
			# Set parameters
			var component = node.get_component()
			if component and comp_data.has("parameters"):
				for param_name in comp_data.parameters:
					component.set_parameter(param_name, comp_data.parameters[param_name])
	
	# Create connections
	for conn in data.connections:
		graph_edit.connect_node(conn.from, conn.from_port, conn.to, conn.to_port)
	
	status_label.text = "Loaded workspace from: " + path.get_file()

func _on_connection_request(from_node: StringName, from_port: int, 
						  to_node: StringName, to_port: int) -> void:
	graph_edit.connect_node(from_node, from_port, to_node, to_port)
	has_unsaved_changes = true

func _on_disconnection_request(from_node: StringName, from_port: int,
							 to_node: StringName, to_port: int) -> void:
	graph_edit.disconnect_node(from_node, from_port, to_node, to_port)
	has_unsaved_changes = true

func _process(_delta: float) -> void:
	if simulation_running:
		simulation_view.update_simulation()
