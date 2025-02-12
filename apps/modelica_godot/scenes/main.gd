class_name ModelicaMain
extends Control

# Node references
@onready var graph_edit: GraphEdit = %GraphEdit
@onready var component_list: VBoxContainer = %ComponentList
@onready var simulation_controls: HBoxContainer = %SimulationControls

# Buttons
@onready var play_button: Button = %PlayButton
@onready var pause_button: Button = %PauseButton
@onready var step_button: Button = %StepButton
@onready var reset_button: Button = %ResetButton

# State
var model_manager: ModelManager
var dragging_component: bool = false
var dragging_type: String = ""

func _ready() -> void:
	model_manager = ModelManager.new()
	add_child(model_manager)
	
	_setup_component_list()
	_connect_signals()

func _setup_component_list() -> void:
	var mechanical_components = {
		"Mass": MassComponent,
		"Spring": SpringComponent,
		"Ground": GroundComponent
	}
	
	var electrical_components = {
		"VoltageSource": VoltageSourceComponent,
		"Resistor": ResistorComponent
	}
	
	var mechanical_list = %ComponentList.get_node("MechanicalSection/MechanicalList")
	var electrical_list = %ComponentList.get_node("ElectricalSection/ElectricalList")
	
	for component_name in mechanical_components:
		var button = Button.new()
		button.text = component_name
		button.custom_minimum_size.y = 30
		mechanical_list.add_child(button)
		button.pressed.connect(_on_component_button_pressed.bind(component_name))
	
	for component_name in electrical_components:
		var button = Button.new()
		button.text = component_name
		button.custom_minimum_size.y = 30
		electrical_list.add_child(button)
		button.pressed.connect(_on_component_button_pressed.bind(component_name))

func _connect_signals() -> void:
	graph_edit.connection_request.connect(_on_connection_request)
	graph_edit.disconnection_request.connect(_on_disconnection_request)
	
	play_button.pressed.connect(_on_play_pressed)
	pause_button.pressed.connect(_on_pause_pressed)
	step_button.pressed.connect(_on_step_pressed)
	reset_button.pressed.connect(_on_reset_pressed)

func _input(event: InputEvent) -> void:
	if event is InputEventMouseButton:
		if event.button_index == MOUSE_BUTTON_LEFT:
			if !event.pressed and dragging_component:
				_create_component(dragging_type, get_local_mouse_position())
				dragging_component = false
				dragging_type = ""

func _create_component(type: String, position: Vector2) -> void:
	var component_class = _get_component_class(type)
	if component_class:
		var component = component_class.new()
		model_manager.add_component(component)
		
		var graph_node = _create_graph_node(component)
		graph_node.position_offset = graph_edit.get_local_mouse_position()
		graph_edit.add_child(graph_node)

func _get_component_class(type: String) -> GDScript:
	match type:
		"Mass": return MassComponent
		"Spring": return SpringComponent
		"Ground": return GroundComponent
		"VoltageSource": return VoltageSourceComponent
		"Resistor": return ResistorComponent
	return null

func _create_graph_node(component: ModelicaComponent) -> ComponentGraphNode:
	match component.get_class():
		"MassComponent": return MassGraphNode.new(component)
		"SpringComponent": return SpringGraphNode.new(component)
		"GroundComponent": return GroundGraphNode.new(component)
		"VoltageSourceComponent", "ResistorComponent": 
			return ElectricalGraphNode.new(component)
	return ComponentGraphNode.new(component)

func _on_component_button_pressed(type: String) -> void:
	dragging_component = true
	dragging_type = type

func _on_connection_request(from_node: StringName, from_port: int, 
						  to_node: StringName, to_port: int) -> void:
	var from_component = model_manager.get_component(from_node)
	var to_component = model_manager.get_component(to_node)
	
	if from_component and to_component:
		if model_manager.connect_components(from_component, str(from_port), 
										 to_component, str(to_port)):
			graph_edit.connect_node(from_node, from_port, to_node, to_port)

func _on_disconnection_request(from_node: StringName, from_port: int,
							 to_node: StringName, to_port: int) -> void:
	# TODO: Implement disconnection
	pass

func _on_play_pressed() -> void:
	model_manager.simulate(0.1)
	_update_visualization()

func _on_pause_pressed() -> void:
	# TODO: Implement pause
	pass

func _on_step_pressed() -> void:
	model_manager.simulate(model_manager.dt)
	_update_visualization()

func _on_reset_pressed() -> void:
	# TODO: Implement reset
	pass

func _update_visualization() -> void:
	for component in model_manager.components:
		# Convert component name to NodePath
		var node_path = NodePath(component.name)
		var graph_node = graph_edit.get_node_or_null(node_path)
		if graph_node and graph_node.has_method("update_visualization"):
			graph_node.update_visualization() 
