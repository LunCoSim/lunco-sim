@tool
extends Control

@onready var model_manager: ModelManager = $ModelManager
@onready var model_browser_window: ModelBrowserWindow = $ModelBrowserWindow
@onready var graph_edit: GraphEdit = $UI/HSplitContainer/VSplitContainer/GraphEdit
@onready var status_label: Label = $UI/Toolbar/HBoxContainer/StatusLabel
@onready var file_menu: MenuButton = $UI/Toolbar/HBoxContainer/FileMenuBtn
@onready var simulation_view = $UI/HSplitContainer/SimulationView

var component_count: int = 0
var current_file: String = ""
var has_unsaved_changes: bool = false
var is_simulating: bool = false

func _ready() -> void:
	print("Starting main scene")
	
	if not model_manager:
		push_error("ModelManager node not found")
		return
		
	if not model_browser_window:
		push_error("ModelBrowserWindow node not found")
		return
		
	if not graph_edit:
		push_error("GraphEdit node not found")
		return
	
	print("Main scene: Initializing model browser with model manager")
	# Initialize model browser window
	model_browser_window.initialize(model_manager)
	model_browser_window.model_selected.connect(_on_model_selected)
	
	# Connect GraphEdit signals
	graph_edit.connection_request.connect(_on_connection_request)
	graph_edit.disconnection_request.connect(_on_disconnection_request)
	
	# Set GraphEdit properties
	graph_edit.snapping_enabled = true
	graph_edit.snapping_distance = 20
	graph_edit.show_grid = true
	
	# Connect to model manager signals
	model_manager.models_loaded_changed.connect(_on_models_loaded)
	model_manager.loading_progress.connect(_on_loading_progress)
	
	# Setup file menu
	_setup_file_menu()

	print("Main scene initialized successfully")

func _setup_file_menu() -> void:
	var popup = file_menu.get_popup()
	popup.id_pressed.connect(_on_file_menu_item_selected)

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
	
	if current_file:
		file_dialog.current_path = current_file
	else:
		file_dialog.current_path = "workspace.json"
	
	file_dialog.title = "Save Workspace"
	add_child(file_dialog)
	file_dialog.popup_centered(Vector2(800, 600))
	
	file_dialog.file_selected.connect(
		func(path):
			_save_workspace_to_file(path)
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
			_load_workspace_from_file(path)
			file_dialog.queue_free()
	)

func _check_dependencies() -> bool:
	# Check if all required packages are available
	var required_packages = [
		"Mechanical",  # Base mechanical package
		"Mechanical.Basic",  # Basic components
		"Mechanical.Interfaces"  # Connectors
	]
	
	for package in required_packages:
		if not model_manager.has_package(package):
			status_label.text = "Missing package: " + package
			return false
	return true

func _import_modelica_file(path: String) -> void:
	# Check dependencies first
	if not _check_dependencies():
		status_label.text = "Missing required dependencies"
		return
		
	var file = FileAccess.open(path, FileAccess.READ)
	if not file:
		status_label.text = "Error importing Modelica file"
		return
	
	var content = file.get_as_text()
	
	# Parse the Modelica file and create components
	var model_name = path.get_file().get_basename()
	_create_model_from_modelica(content, model_name)
	
	status_label.text = "Imported Modelica model from " + path.get_file()

func _export_modelica_file(path: String) -> void:
	var model_code = _generate_modelica_code()
	
	var file = FileAccess.open(path, FileAccess.WRITE)
	if file:
		file.store_string(model_code)
		status_label.text = "Exported Modelica model to " + path.get_file()
	else:
		status_label.text = "Error exporting Modelica model"

func _generate_modelica_code() -> String:
	var model_name = current_file.get_file().get_basename() if current_file else "GeneratedModel"
	var code = "model " + model_name + "\n"
	
	# Declare components
	for node in graph_edit.get_children():
		if not (node is GraphNode):
			continue
			
		var comp_name = node.name.to_lower()
		var comp_type = node.component_type
		var comp_data = node.component_data
		
		# Component declaration
		code += "  " + comp_type + " " + comp_name
		
		# Component parameters
		var params = []
		for param in comp_data:
			params.append(param + "=" + str(comp_data[param]))
		
		if not params.is_empty():
			code += "(" + ", ".join(params) + ")"
		
		code += ";\n"
	
	# Add equations for connections
	code += "\nequation\n"
	var connections = graph_edit.get_connection_list()
	for conn in connections:
		code += "  connect(" 
		code += conn["from_node"].to_lower() + ".port"
		if "port_b" in conn["from_node"].to_lower():
			code += "_b"
		code += ", "
		code += conn["to_node"].to_lower() + ".port"
		if "port_b" in conn["to_node"].to_lower():
			code += "_b"
		code += ");\n"
	
	code += "end " + model_name + ";\n"
	return code

func _create_model_from_modelica(content: String, model_name: String) -> void:
	# This is a simple parser for demonstration
	# In practice, you'd want a more robust Modelica parser
	
	# Clear current system
	_new_system()
	
	var lines = content.split("\n")
	var in_equation_section = false
	var components = {}
	
	for line in lines:
		line = line.strip_edges()
		
		if line.begins_with("model "):
			continue
		elif line.begins_with("equation"):
			in_equation_section = true
			continue
		elif line.begins_with("end "):
			break
		
		if not in_equation_section:
			# Parse component declarations
			if line.ends_with(";"):
				line = line.trim_suffix(";")
				var parts = line.split(" ", false)
				if parts.size() >= 2:
					var comp_type = parts[0].strip_edges()
					var comp_name = parts[1].strip_edges()
					
					# Create component
					_create_component(comp_type, comp_name)
					components[comp_name] = comp_type
		else:
			# Parse connections
			if line.begins_with("connect("):
				line = line.trim_prefix("connect(").trim_suffix(");")
				var ports = line.split(",", false)
				if ports.size() == 2:
					_create_connection(ports[0].strip_edges(), ports[1].strip_edges())

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

func _create_connection(from_port: String, to_port: String) -> void:
	var from_parts = from_port.split(".")
	var to_parts = to_port.split(".")
	
	if from_parts.size() == 2 and to_parts.size() == 2:
		var from_node = from_parts[0]
		var to_node = to_parts[0]
		
		# Find the nodes and connect them
		var from = graph_edit.get_node_or_null(NodePath(from_node))
		var to = graph_edit.get_node_or_null(NodePath(to_node))
		
		if from and to:
			graph_edit.connect_node(from.name, 0, to.name, 0)

func _save_workspace_to_file(path: String) -> void:
	var system_data = {
		"nodes": [],
		"connections": []
	}
	
	# Save nodes
	for node in graph_edit.get_children():
		if node is GraphNode:
			var node_data = {
				"type": node.component_type,
				"data": node.component_data,
				"position": node.position_offset,
				"name": node.name
			}
			system_data["nodes"].append(node_data)
	
	# Save connections
	var connections = graph_edit.get_connection_list()
	system_data["connections"] = connections
	
	# Save to file
	var file = FileAccess.open(path, FileAccess.WRITE)
	if file:
		file.store_string(JSON.stringify(system_data, "  "))
		current_file = path
		has_unsaved_changes = false
		status_label.text = "Workspace saved to " + path.get_file()
	else:
		status_label.text = "Error saving workspace"

func _load_workspace_from_file(path: String) -> void:
	var file = FileAccess.open(path, FileAccess.READ)
	if not file:
		status_label.text = "Error loading workspace"
		return
	
	var json = JSON.new()
	var error = json.parse(file.get_as_text())
	if error != OK:
		status_label.text = "Error parsing workspace file"
		return
	
	var system_data = json.get_data()
	
	# Clear current system
	_new_system()
	
	# Create nodes
	for node_data in system_data["nodes"]:
		var node = GraphNode.new()
		node.set_script(load("res://apps/modelica_godot/ui/component_node.gd"))
		node.name = node_data["name"]
		node.position_offset = Vector2(node_data["position"]["x"], node_data["position"]["y"])
		graph_edit.add_child(node)
		node.setup(node_data["type"], node_data["data"])
	
	# Create connections
	for connection in system_data["connections"]:
		graph_edit.connect_node(
			connection["from_node"],
			connection["from_port"],
			connection["to_node"],
			connection["to_port"]
		)
	
	current_file = path
	has_unsaved_changes = false
	status_label.text = "Workspace loaded from " + path.get_file()

func _on_models_loaded() -> void:
	print("Main scene: Models loaded")
	print("Model tree size: ", model_manager._model_tree.size())
	print("Model tree contents: ", model_manager._model_tree.keys())
	status_label.text = "Models loaded"

func _on_loading_progress(progress: float, message: String) -> void:
	status_label.text = message

func _on_model_selected(model_path: String, model_data: Dictionary) -> void:
	print("Selected model: ", model_path)
	# TODO: Handle model selection

func _on_library_pressed():
	model_browser_window.show()

func _on_component_selected(type: String) -> void:
	# Create a new component node
	var node = GraphNode.new()
	node.name = type + str(component_count)
	node.title = type
	node.position_offset = Vector2(100, 100) * (component_count + 1)
	node.size = Vector2(200, 100)
	node.set_script(load("res://apps/modelica_godot/ui/component_node.gd"))
	
	# Add to graph
	graph_edit.add_child(node)
	
	# Initialize the node
	node.setup(type)
	
	# Increment counter for offset positioning
	component_count += 1
	has_unsaved_changes = true
	
	status_label.text = "Added " + type + " component"

func _on_connection_request(from_node: StringName, from_port: int, 
						  to_node: StringName, to_port: int) -> void:
	# Check if connection is valid
	if _can_connect(from_node, to_node):
		graph_edit.connect_node(from_node, from_port, to_node, to_port)
		has_unsaved_changes = true
		status_label.text = "Connected components"
	else:
		status_label.text = "Invalid connection"

func _on_disconnection_request(from_node: StringName, from_port: int,
							 to_node: StringName, to_port: int) -> void:
	graph_edit.disconnect_node(from_node, from_port, to_node, to_port)
	has_unsaved_changes = true
	status_label.text = "Disconnected components"

func _can_connect(from_node: StringName, to_node: StringName) -> bool:
	# Get the actual nodes
	var from = graph_edit.get_node_or_null(NodePath(from_node))
	var to = graph_edit.get_node_or_null(NodePath(to_node))
	
	if not from or not to:
		return false
	
	# Don't connect a node to itself
	if from == to:
		return false
	
	return true

func _on_simulate_pressed() -> void:
	if is_simulating:
		return
		
	is_simulating = true
	status_label.text = "Simulating..."
	
	# Initialize equation system
	model_manager.equation_system.initialize()
	
	# Connect simulation view to equation system
	simulation_view.set_equation_system(model_manager.equation_system)

func _on_stop_pressed() -> void:
	is_simulating = false
	status_label.text = "Simulation stopped"

func _on_load_msl_pressed() -> void:
	model_browser_window.show()
	model_browser_window.get_node("ModelBrowser").load_msl()

func _process(delta: float) -> void:
	if is_simulating:
		# Update simulation using equation system
		model_manager.equation_system.solve_step()
		
		# Update status
		status_label.text = "Time: %.2f s" % model_manager.equation_system.time
