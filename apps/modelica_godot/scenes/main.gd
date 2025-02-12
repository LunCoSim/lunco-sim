@tool
extends Control

@onready var model_manager: ModelManager = $ModelManager
@onready var model_browser_window: ModelBrowserWindow = $ModelBrowserWindow
@onready var graph_edit: GraphEdit = $UI/GraphEdit
@onready var status_label: Label = $UI/Toolbar/HBoxContainer/StatusLabel

var component_count: int = 0

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

	print("Main scene initialized successfully")

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
