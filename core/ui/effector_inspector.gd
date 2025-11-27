class_name LCEffectorInspector
extends Window

## Main UI for inspecting and controlling vehicle effectors.
##
## Automatically discovers effectors on the target vehicle and creates
## panels for them.

@export var vehicle_path: NodePath
var vehicle: LCVehicle

var scroll_container: ScrollContainer
var grid_container: GridContainer

func _ready():
	# Setup Window properties
	title = "Effector Inspector"
	initial_position = Window.WINDOW_INITIAL_POSITION_CENTER_MAIN_WINDOW_SCREEN
	size = Vector2i(600, 500)
	visible = false
	close_requested.connect(func(): hide())
	
	# Main layout
	# Add margin
	var margin = MarginContainer.new()
	margin.size_flags_vertical = Control.SIZE_EXPAND_FILL
	margin.add_theme_constant_override("margin_top", 10)
	margin.add_theme_constant_override("margin_left", 10)
	margin.add_theme_constant_override("margin_right", 10)
	margin.add_theme_constant_override("margin_bottom", 10)
	main_vbox.add_child(margin)
	
	var content_vbox = VBoxContainer.new()
	content_vbox.size_flags_vertical = Control.SIZE_EXPAND_FILL
	margin.add_child(content_vbox)
	
	# Vehicle Selector (if no vehicle assigned)
	if vehicle_path:
		vehicle = get_node(vehicle_path)
		if vehicle:
			_setup_for_vehicle(vehicle)
			
	# Connect to BuilderManager for selection updates
	if BuilderManager:
		BuilderManager.entity_selected.connect(_on_entity_selected)
	
	# Scroll Area
	scroll_container = ScrollContainer.new()
	scroll_container.size_flags_vertical = Control.SIZE_EXPAND_FILL
	content_vbox.add_child(scroll_container)
	
	grid_container = GridContainer.new()
	grid_container.columns = 2
	grid_container.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	scroll_container.add_child(grid_container)

func set_vehicle(target_vehicle: LCVehicle):
	vehicle = target_vehicle
	if vehicle:
		_setup_for_vehicle(vehicle)
	else:
		# Clear if null
		for child in grid_container.get_children():
			child.queue_free()

func _setup_for_vehicle(target: LCVehicle):
	if not target:
		return
		
	# Clear existing panels
	for child in grid_container.get_children():
		child.queue_free()
	
	# Find all effectors
	var effectors = []
	effectors.append_array(target.state_effectors)
	effectors.append_array(target.dynamic_effectors)
	
	# Remove duplicates (hybrid effectors)
	var unique_effectors = []
	for eff in effectors:
		if not unique_effectors.has(eff):
			unique_effectors.append(eff)
	
	# Create panels
	for eff in unique_effectors:
		var panel = LCEffectorPanel.new()
		grid_container.add_child(panel)
		panel.setup(eff)

func _process(delta):
	# Auto-find vehicle if not set (e.g. for testing)
	if not vehicle and vehicle_path:
		var node = get_node_or_null(vehicle_path)
		if node and node is LCVehicle:
			set_vehicle(node)


func _on_entity_selected(entity):
	if entity is LCVehicle:
		set_vehicle(entity)
	elif entity is LCConstructible:
		# LCConstructible might not be compatible with LCVehicle-based inspector
		# unless we make them compatible or check for components
		# For now, we only support LCVehicle (the new system)
		pass
