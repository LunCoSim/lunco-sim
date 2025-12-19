class_name GraphView
extends GraphEdit

const MODULE_PATH = "res://apps/supply_chain_modeling"

# Native GraphEdit node_selected signal is used instead of custom implementation

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
		"regolith_reduction_reactor", "RegolithReductionReactor":
			ui_node = load(MODULE_PATH + "/ui/facilities/ui_regolith_reduction_reactor.tscn").instantiate()
		"water_collection_system", "WaterCollectionSystem":
			ui_node = load(MODULE_PATH + "/ui/facilities/ui_water_collection_system.tscn").instantiate()
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

var current_solver_graph: LCSolverGraph
var edge_connection_map = {} # edge_id -> {from, from_port, to, to_port}

func _process(_delta):
	if not visible or not current_solver_graph:
		return
		
	# Update connection flow animations
	for edge_id in current_solver_graph.edges:
		var edge = current_solver_graph.edges[edge_id]
		var conn_info = edge_connection_map.get(edge_id)
		
		if conn_info:
			# Calculate activity based on flow rate
			# Activity controls the speed of the scrolling texture on the connection line
			var flow_mag = abs(edge.flow_rate)
			
			if flow_mag > 0.001:
				# Logarithmic scaling for visualization range so both small and large flows are visible
				# + 0.1 ensures even small flows move a bit
				var activity = (log(flow_mag + 1.0) * 0.5) + 0.1
				
				# Reverse animation if flow is negative (if that's possible in the solver logic)
				# Though typically flow_rate is magnitude or direction relative to defined A->B
				# If flow is from B to A, flow_rate might be negative.
				if edge.flow_rate < 0:
					activity = -activity
					
				set_connection_activity(conn_info.from, conn_info.from_port, conn_info.to, conn_info.to_port, activity)
			else:
				set_connection_activity(conn_info.from, conn_info.from_port, conn_info.to, conn_info.to_port, 0.0)

## Load and visualize a raw LCSolverGraph
func load_from_solver_graph(graph: LCSolverGraph):
	clear_graph()
	clear_connections()
	
	current_solver_graph = graph
	edge_connection_map.clear()
	
	if not graph:
		return
		
	var node_map = {} # solver_id -> ui_node_name
	var layout_x = 100
	var layout_y = 100
	var spacing = 250
	var cols = 4
	var i = 0
	
	# Create Nodes
	for node_id in graph.nodes:
		var solver_node: LCSolverNode = graph.nodes[node_id]
		var ui_node = load(MODULE_PATH + "/ui/widgets/ui_solver_node.tscn").instantiate()
		
		ui_node.solver_node = solver_node
		ui_node.name = "SolverNode_" + str(node_id)
		node_map[node_id] = ui_node.name
		
		# Simple grid layout
		ui_node.position_offset = Vector2(layout_x + (i % cols) * spacing, layout_y + (i / cols) * spacing)
		i += 1
		
		add_child(ui_node)
	
	# Create Connections
	for edge_id in graph.edges:
		var edge: LCSolverEdge = graph.edges[edge_id]
		var from_name = node_map.get(edge.node_a.id)
		var to_name = node_map.get(edge.node_b.id)
		
		if from_name and to_name:
			connect_node(from_name, 0, to_name, 0)
			
			# Store connection info for animation
			edge_connection_map[edge_id] = {
				"from": from_name,
				"from_port": 0,
				"to": to_name,
				"to_port": 0
			}
