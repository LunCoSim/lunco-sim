class_name LCResourceMonitor
extends PanelContainer

## UI panel for monitoring vehicle resources
##
## Displays current amounts and capacities of resources in the vehicle's tanks.

@export var vehicle_path: NodePath
var vehicle: LCVehicle

var container: VBoxContainer
var resource_bars: Dictionary = {}  # resource_id -> ProgressBar

func _ready():
	visible = false
	# Setup layout
	container = VBoxContainer.new()
	add_child(container)
	
	var label = Label.new()
	label.text = "Resources"
	label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	container.add_child(label)
	
	container.add_child(HSeparator.new())
	
	# Auto-find vehicle if path set
	if vehicle_path:
		var node = get_node_or_null(vehicle_path)
		if node and node is LCVehicle:
			set_vehicle(node)
	
	# Connect to builder manager for selection updates
	if BuilderManager:
		BuilderManager.entity_selected.connect(_on_entity_selected)

func set_vehicle(target: LCVehicle):
	vehicle = target
	visible = (vehicle != null)
	_rebuild_ui()

func _on_entity_selected(entity):
	if entity is LCVehicle:
		set_vehicle(entity)
	else:
		set_vehicle(null)

func _rebuild_ui():
	# Clear existing bars
	for child in container.get_children():
		if child is HBoxContainer: # Resource rows
			child.queue_free()
	resource_bars.clear()
	
	if not vehicle or not vehicle.resource_network:
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

func _process(delta):
	if not vehicle or not vehicle.resource_network:
		return
	
	for res_id in resource_bars:
		var bar = resource_bars[res_id]
		var total = vehicle.resource_network.get_total_resource(res_id)
		var capacity = vehicle.resource_network.get_total_capacity(res_id)
		
		if capacity > 0:
			bar.value = (total / capacity) * 100.0
			bar.tooltip_text = "%.1f / %.1f" % [total, capacity]
