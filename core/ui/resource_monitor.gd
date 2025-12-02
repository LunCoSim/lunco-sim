class_name LCResourceMonitor
extends PanelContainer

## UI panel for monitoring vehicle resources
##
## Displays current amounts and capacities of resources in the vehicle's tanks.

@export var vehicle_path: NodePath
var vehicle: Node # Can be LCVehicle or LCSpacecraft

var container: VBoxContainer
var resource_bars: Dictionary = {}  # resource_id -> ProgressBar

func _ready():
	# Setup layout
	container = VBoxContainer.new()
	add_child(container)
	
	# Title for the section
	var title_label = Label.new()
	title_label.name = "HeaderTitle"
	title_label.text = "Resources"
	title_label.add_theme_font_size_override("font_size", 16)
	title_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_LEFT
	container.add_child(title_label)
	
	var sep = HSeparator.new()
	sep.name = "HeaderSeparator"
	container.add_child(sep)
	
	# Auto-find vehicle if path set
	if vehicle_path:
		var node = get_node_or_null(vehicle_path)
		if node and (node is LCVehicle or node is LCSpacecraft):
			set_vehicle(node)
	
	# Connect to builder manager for selection updates
	if BuilderManager:
		BuilderManager.entity_selected.connect(_on_entity_selected)

func set_vehicle(target: Node):
	# Disconnect from old vehicle
	if vehicle and is_instance_valid(vehicle) and "resource_network" in vehicle and vehicle.resource_network:
		if vehicle.resource_network.network_updated.is_connected(_on_network_updated):
			vehicle.resource_network.network_updated.disconnect(_on_network_updated)
			
	vehicle = target
	
	# Connect to new vehicle
	if vehicle and "resource_network" in vehicle and vehicle.resource_network:
		if not vehicle.resource_network.network_updated.is_connected(_on_network_updated):
			vehicle.resource_network.network_updated.connect(_on_network_updated)
	
	# Always visible when embedded, just show empty state if no vehicle
	_rebuild_ui()

func _on_entity_selected(entity):
	if entity is LCVehicle or entity is LCSpacecraft:
		set_vehicle(entity)
	else:
		set_vehicle(null)

func _rebuild_ui():
	# Clear existing bars
	for child in container.get_children():
		if child.name != "HeaderTitle" and child.name != "HeaderSeparator":
			child.queue_free()
	resource_bars.clear()
	
	if not vehicle:
		# Show "no vehicle selected" message
		var no_vehicle_label = Label.new()
		no_vehicle_label.name = "NoVehicleLabel"
		no_vehicle_label.text = "No vehicle selected"
		no_vehicle_label.add_theme_color_override("font_color", Color(0.7, 0.7, 0.7))
		container.add_child(no_vehicle_label)
		return
	
	if not "resource_network" in vehicle or not vehicle.resource_network:
		# Show "no resource network" message
		var no_network_label = Label.new()
		no_network_label.name = "NoVehicleLabel"
		no_network_label.text = "Vehicle has no resource network"
		no_network_label.add_theme_color_override("font_color", Color(0.9, 0.7, 0.3))
		container.add_child(no_network_label)
		return
	
	# Find all resources in the network
	var resources = vehicle.resource_network.resource_types.keys()
	
	for res_id in resources:
		# Only show if there are tanks (capacity > 0)
		var capacity = vehicle.resource_network.get_total_capacity(res_id)
		if capacity <= 0:
			continue
			
		var res_def = LCResourceRegistry.get_resource(res_id)
		var res_name = res_def.display_name if res_def else res_id
		
		var row = HBoxContainer.new()
		container.add_child(row)
		
		var name_lbl = Label.new()
		name_lbl.text = res_name
		name_lbl.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		row.add_child(name_lbl)
		
		var bar = ProgressBar.new()
		bar.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		bar.size_flags_stretch_ratio = 2.0
		bar.show_percentage = true
		row.add_child(bar)
		
		resource_bars[res_id] = bar

func _on_network_updated():
	if not vehicle or not "resource_network" in vehicle or not vehicle.resource_network:
		return
		
	for res_id in resource_bars:
		var bar = resource_bars[res_id]
		var total = vehicle.resource_network.get_total_resource(res_id)
		var capacity = vehicle.resource_network.get_total_capacity(res_id)
		
		if capacity > 0:
			bar.value = (total / capacity) * 100.0
			bar.tooltip_text = "%.1f / %.1f" % [total, capacity]
