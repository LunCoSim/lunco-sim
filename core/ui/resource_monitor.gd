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
	vehicle = target
	
	# Always visible when embedded, just show empty state if no vehicle
	_rebuild_ui()
	
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
	
	if not "solver_graph" in vehicle or not vehicle.solver_graph:
		# Show "no solver graph" message
		var no_graph_label = Label.new()
		no_graph_label.name = "NoGraphLabel"
		no_graph_label.text = "Vehicle has no solver graph"
		no_graph_label.add_theme_color_override("font_color", Color(0.9, 0.7, 0.3))
		container.add_child(no_graph_label)
		return
	
	# Find all fluid storage nodes in the solver graph
	var fluid_nodes = {}  # resource_type -> {total, capacity}
	
	for node_id in vehicle.solver_graph.nodes:
		var node = vehicle.solver_graph.nodes[node_id]
		if node.domain == "Fluid" and node.is_storage and node.capacitance > 0:
			var res_type = node.resource_type if node.resource_type else "unknown"
			if not res_type in fluid_nodes:
				fluid_nodes[res_type] = {"total": 0.0, "capacity": 0.0}
			
			# flow_accumulation is mass in kg
			fluid_nodes[res_type]["total"] += node.flow_accumulation
			# capacitance is the storage capacity
			fluid_nodes[res_type]["capacity"] += node.capacitance
	
	if fluid_nodes.is_empty():
		var no_resources_label = Label.new()
		no_resources_label.text = "No fluid resources"
		no_resources_label.add_theme_color_override("font_color", Color(0.7, 0.7, 0.7))
		container.add_child(no_resources_label)
		return
	
	for res_id in fluid_nodes:
		var data = fluid_nodes[res_id]
		var registry = get_node_or_null("/root/LCResourceRegistry")
		var res_def = registry.get_resource(res_id) if registry else null
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

var update_timer: float = 0.0

func _process(delta):
	# Throttle updates to 10Hz
	update_timer += delta
	if update_timer > 0.1:
		update_timer = 0.0
		_update_bars()

func _update_bars():
	if not vehicle or not "solver_graph" in vehicle or not vehicle.solver_graph:
		return
	
	# Update bars from solver graph
	var fluid_nodes = {}  # resource_type -> {total, capacity}
	
	for node_id in vehicle.solver_graph.nodes:
		var node = vehicle.solver_graph.nodes[node_id]
		if node.domain == "Fluid" and node.is_storage and node.capacitance > 0:
			var res_type = node.resource_type if node.resource_type else "unknown"
			if not res_type in fluid_nodes:
				fluid_nodes[res_type] = {"total": 0.0, "capacity": 0.0}
			
			fluid_nodes[res_type]["total"] += node.flow_accumulation
			fluid_nodes[res_type]["capacity"] += node.capacitance
	
	for res_id in resource_bars:
		if res_id in fluid_nodes:
			var bar = resource_bars[res_id]
			var data = fluid_nodes[res_id]
			
			if data["capacity"] > 0:
				bar.value = (data["total"] / data["capacity"]) * 100.0
				bar.tooltip_text = "%.1f / %.1f kg" % [data["total"], data["capacity"]]
