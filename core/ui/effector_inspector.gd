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
var resource_monitor: LCResourceMonitor

func _ready():
	# Setup Window properties
	title = "Vehicle Inspector"
	initial_position = Window.WINDOW_INITIAL_POSITION_CENTER_MAIN_WINDOW_SCREEN
	size = Vector2i(600, 600)
	visible = false
	close_requested.connect(func(): hide())
	visibility_changed.connect(_on_visibility_changed)
	
	# Main layout
	var main_vbox = VBoxContainer.new()
	main_vbox.size_flags_vertical = Control.SIZE_EXPAND_FILL
	add_child(main_vbox)
	
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
	
	# Resource Monitor Section
	resource_monitor = LCResourceMonitor.new()
	resource_monitor.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	content_vbox.add_child(resource_monitor)
	
	# Separator
	var separator = HSeparator.new()
	separator.add_theme_constant_override("separation", 10)
	content_vbox.add_child(separator)
	
	# Effectors Label
	var effectors_label = Label.new()
	effectors_label.text = "Effectors"
	effectors_label.add_theme_font_size_override("font_size", 16)
	content_vbox.add_child(effectors_label)
	
	# Vehicle Selector (if no vehicle assigned)
	if vehicle_path:
		vehicle = get_node(vehicle_path)
		if vehicle:
			_setup_for_vehicle(vehicle)
			
	# Connect to BuilderManager for selection updates
	if BuilderManager:
		BuilderManager.entity_selected.connect(_on_entity_selected)
	
	# Scroll Area for Effectors
	scroll_container = ScrollContainer.new()
	scroll_container.size_flags_vertical = Control.SIZE_EXPAND_FILL
	content_vbox.add_child(scroll_container)
	
	grid_container = GridContainer.new()
	grid_container.columns = 2
	grid_container.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	scroll_container.add_child(grid_container)

func set_vehicle(target_vehicle: LCVehicle):
	vehicle = target_vehicle
	
	# Update resource monitor
	if resource_monitor:
		resource_monitor.set_vehicle(vehicle)
	
	if vehicle:
		_setup_for_vehicle(vehicle)
	else:
		# Clear if null
		for child in grid_container.get_children():
			child.queue_free()

func _setup_for_vehicle(target: LCVehicle):
	# Clear existing panels
	for child in grid_container.get_children():
		child.queue_free()
	
	if not target:
		# Show "no vehicle selected" message
		var no_vehicle_label = Label.new()
		no_vehicle_label.text = "No vehicle selected"
		no_vehicle_label.add_theme_color_override("font_color", Color(0.7, 0.7, 0.7))
		grid_container.add_child(no_vehicle_label)
		return
	
	# Find all effectors
	var effectors = []
	effectors.append_array(target.state_effectors)
	effectors.append_array(target.dynamic_effectors)
	
	# Remove duplicates (hybrid effectors)
	var unique_effectors = []
	for eff in effectors:
		if not unique_effectors.has(eff):
			unique_effectors.append(eff)
	
	# Show message if no effectors found
	if unique_effectors.is_empty():
		var no_effectors_label = Label.new()
		no_effectors_label.text = "No effectors found on vehicle"
		no_effectors_label.add_theme_color_override("font_color", Color(0.9, 0.7, 0.3))
		grid_container.add_child(no_effectors_label)
		return
	
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

func _on_visibility_changed():
	# When the window becomes visible, try to find the current vehicle
	if visible and not vehicle:
		_try_find_current_vehicle()

func _try_find_current_vehicle():
	# Try to get the vehicle from the avatar's current target
	var avatar = _find_avatar()
	if avatar and avatar.target:
		var entity = avatar.target
		# If target is a controller, get its parent (the vehicle)
		if entity is LCController:
			entity = entity.get_parent()
		
		# Check if it's a vehicle
		if entity is LCVehicle:
			set_vehicle(entity)
			return
	
	# Fallback: try to find any vehicle in the scene
	var root = get_tree().root
	var vehicles = _find_vehicles_recursive(root)
	if not vehicles.is_empty():
		set_vehicle(vehicles[0])

func _find_avatar() -> Node:
	# Try to find the avatar in the scene
	var ui = get_parent()
	if ui and ui.get_parent():
		return ui.get_parent()
	return null

func _find_vehicles_recursive(node: Node) -> Array:
	var vehicles = []
	if node is LCVehicle:
		vehicles.append(node)
	for child in node.get_children():
		vehicles.append_array(_find_vehicles_recursive(child))
	return vehicles


func _on_entity_selected(entity):
	if entity is LCVehicle:
		set_vehicle(entity)
	elif entity is LCConstructible:
		# LCConstructible might not be compatible with LCVehicle-based inspector
		# unless we make them compatible or check for components
		# For now, we only support LCVehicle (the new system)
		pass
