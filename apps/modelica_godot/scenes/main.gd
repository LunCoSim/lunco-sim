@tool
extends Control

@onready var model_manager: ModelManager = $ModelManager
@onready var model_browser: ModelBrowser = $UI/HSplitContainer/ModelBrowser
@onready var graph_edit: GraphEdit = $UI/HSplitContainer/GraphEdit
@onready var status_label: Label = $UI/Toolbar/HBoxContainer/StatusLabel

var component_count: int = 0

func _ready() -> void:
	print("Starting main scene")
	
	if not model_manager:
		push_error("ModelManager node not found")
		return
		
	if not model_browser:
		push_error("ModelBrowser node not found")
		return
		
	if not graph_edit:
		push_error("GraphEdit node not found")
		return
	
	print("Main scene: Initializing model browser with model manager")
	# Initialize model browser
	model_browser.initialize(model_manager)
	model_browser.model_selected.connect(_on_model_selected)
	
	# Connect GraphEdit signals
	graph_edit.connection_request.connect(_on_connection_request)
	graph_edit.disconnection_request.connect(_on_disconnection_request)
	
	# Set GraphEdit properties
	graph_edit.snapping_enabled = true
	graph_edit.snapping_distance = 20
	graph_edit.show_grid = true
	
	# Connect to model manager signals
	model_manager.models_loaded.connect(_on_models_loaded)
	model_manager.loading_progress.connect(_on_loading_progress)

	print("Main scene initialized successfully")

func _on_models_loaded() -> void:
	print("Main scene: Models loaded")
	print("Model tree size: ", model_manager._model_tree.size())
	print("Model tree contents: ", model_manager._model_tree.keys())
	status_label.text = "Models loaded"

func _on_loading_progress(progress: float, message: String) -> void:
	status_label.text = message

func _on_model_selected(model_path: String, model_data: Dictionary) -> void:
	# Create a new GraphNode for the selected model
	var node = _create_component_node(model_data)
	if node:
		graph_edit.add_child(node)
		status_label.text = "Added " + model_data.get("name", "unnamed")

func _create_component_node(model_data: Dictionary) -> GraphNode:
	var node := GraphNode.new()
	var unique_name = model_data.get("name", "UnknownModel") + str(component_count)
	component_count += 1
	
	# Set up the node
	node.name = unique_name
	node.title = model_data.get("name", "UnknownModel")
	node.position_offset = Vector2(200, 200)  # Default position
	node.draggable = true
	node.resizable = false
	node.size = Vector2(120, 80)  # Set a fixed size
	
	# Create the main container
	var container := VBoxContainer.new()
	container.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	container.size_flags_vertical = Control.SIZE_EXPAND_FILL
	node.add_child(container)
	
	# Add the main body
	var body := ColorRect.new()
	body.custom_minimum_size = Vector2(100, 50)
	body.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	body.size_flags_vertical = Control.SIZE_EXPAND_FILL
	
	# Set color based on component type
	match model_data.get("type", ""):
		"model":
			body.color = Color(0.2, 0.6, 1.0)  # Light blue
		"connector":
			body.color = Color(0.8, 0.2, 0.2)  # Red
		"block":
			body.color = Color(0.2, 0.8, 0.2)  # Green
		_:
			body.color = Color(0.7, 0.7, 0.7)  # Gray
	
	container.add_child(body)
	
	# Add connectors based on model data
	var connectors = model_data.get("connectors", [])
	for i in range(connectors.size()):
		var connector = connectors[i]
		node.set_slot(i,  # Slot index
			true,         # Enable left slot
			0,           # Left slot type
			Color.GOLD,  # Left slot color
			true,        # Enable right slot
			0,           # Right slot type
			Color.GOLD)  # Right slot color
		
		# Add connector label
		var label := Label.new()
		label.text = connector.get("name", "unnamed")
		container.add_child(label)
	
	return node

func _on_connection_request(from_node: StringName, from_port: int, 
						  to_node: StringName, to_port: int) -> void:
	# Check if connection is valid
	if _can_connect(from_node, to_node):
		graph_edit.connect_node(from_node, from_port, to_node, to_port)
		status_label.text = "Connected components"
	else:
		status_label.text = "Invalid connection"

func _on_disconnection_request(from_node: StringName, from_port: int,
							 to_node: StringName, to_port: int) -> void:
	graph_edit.disconnect_node(from_node, from_port, to_node, to_port)
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
	status_label.text = "Simulating..."
	# TODO: Implement simulation

func _on_stop_pressed() -> void:
	status_label.text = "Stopped"
	# TODO: Implement simulation stop
