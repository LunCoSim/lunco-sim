class_name LCEffectorInspector
extends Control

## Main UI for inspecting and controlling vehicle effectors.
##
## Automatically discovers effectors on the target vehicle and creates
## panels for them.

@export var vehicle_path: NodePath
var vehicle: LCVehicle

var scroll_container: ScrollContainer
var grid_container: GridContainer

func _ready():
	# Setup UI layout
	anchor_right = 1.0
	anchor_bottom = 1.0
	
	var main_vbox = VBoxContainer.new()
	main_vbox.set_anchors_preset(PRESET_FULL_RECT)
	add_child(main_vbox)
	
	# Title
	var title = Label.new()
	title.text = "Effector Inspector"
	title.add_theme_font_size_override("font_size", 24)
	title.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	main_vbox.add_child(title)
	
	# Vehicle Selector (if no vehicle assigned)
	if vehicle_path:
		vehicle = get_node(vehicle_path)
		if vehicle:
			_setup_for_vehicle(vehicle)
	
	# Scroll Area
	scroll_container = ScrollContainer.new()
	scroll_container.size_flags_vertical = SIZE_EXPAND_FILL
	main_vbox.add_child(scroll_container)
	
	grid_container = GridContainer.new()
	grid_container.columns = 3
	grid_container.size_flags_horizontal = SIZE_EXPAND_FILL
	scroll_container.add_child(grid_container)

func set_vehicle(target_vehicle: LCVehicle):
	vehicle = target_vehicle
	_setup_for_vehicle(vehicle)

func _setup_for_vehicle(target: LCVehicle):
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
