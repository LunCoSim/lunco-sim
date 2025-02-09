class_name GraphView
extends GraphEdit

const MODULE_PATH = "res://modules/supply_chain_modeling"

func clear_graph():
	for node in get_children():
		if node is GraphNode:
			node.free()

func add_ui_for_node(node: SimulationNode, _position: Vector2 = Vector2.ZERO) -> void:
	if node:
		create_ui_node(node, _position)

func create_ui_node(simulation_node: SimulationNode, _position: Vector2 = Vector2.ZERO) -> GraphNode:
	var ui_node: GraphNode
	var node_class = simulation_node.get_script().get_path().get_file().get_basename()
	
	# Create specific UI node based on simulation node type and properties
	match node_class:
		"storage", "StorageFacility":
			ui_node = load(MODULE_PATH + "/ui/facilities/ui_storage.tscn").instantiate()
		"resource_h2", "ResourceH2":
			ui_node = load(MODULE_PATH + "/ui/resources/ui_resource_h2.tscn").instantiate()
		"resource_o2", "ResourceO2":
			ui_node = load(MODULE_PATH + "/ui/resources/ui_resource_o2.tscn").instantiate()
		"resource_h2o", "ResourceH2O":
			ui_node = load(MODULE_PATH + "/ui/resources/ui_resource_h2o.tscn").instantiate()
		"object_factory", "ObjectFactory":
			ui_node = load(MODULE_PATH + "/ui/facilities/ui_object_factory.tscn").instantiate()
		"solar_power_plant", "SolarPowerPlant":
			ui_node = load(MODULE_PATH + "/ui/facilities/ui_solar_power_plant.tscn").instantiate()
		"pump", "Pump":
			ui_node = load(MODULE_PATH + "/ui/facilities/ui_pump.tscn").instantiate()
		"electrolytic_factory", "ElectrolyticFactory":
			ui_node = load(MODULE_PATH + "/ui/facilities/ui_electrolytic_factory.tscn").instantiate()
		"note_node", "NoteNode":
			ui_node = load(MODULE_PATH + "/ui/other/ui_note_node.tscn").instantiate()
		_:
			# Default UI node if no specific type matches
			print("Unknown node type: ", node_class)
			ui_node = load(MODULE_PATH + "/ui/other/ui_note_node.tscn").instantiate()
	
	# Set common properties
	if ui_node:
		ui_node.simulation_node = simulation_node
		ui_node.name = simulation_node.name
		ui_node.title = node_class
		ui_node.set_physics_process(false)
		
		# Position the node at screen center if not specified
		if _position == Vector2.ZERO:
			var viewport_size = get_viewport_rect().size
			var center_x = (scroll_offset.x + viewport_size.x / 2) / zoom
			var center_y = (scroll_offset.y + viewport_size.y / 2) / zoom
			ui_node.position_offset = Vector2(center_x - ui_node.size.x / 2, center_y - ui_node.size.y / 2)
		else:
			ui_node.position_offset = _position - ui_node.size / 2
	
	add_child(ui_node)
	return ui_node

func get_view_state() -> Dictionary:
	return {
		"scroll_offset": [scroll_offset.x, scroll_offset.y],
		"zoom": zoom
	}

func get_ui_state() -> Dictionary:
	# Save UI node positions
	var save_data = {}

	for node in get_children():
		if node is GraphNode:
			save_data[node.name] = {
				"position": [node.position_offset.x, node.position_offset.y],
				"size": [node.size.x, node.size.y]
			}
			
	return save_data
