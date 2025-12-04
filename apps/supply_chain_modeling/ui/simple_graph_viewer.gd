extends Control

## Lightweight graph viewer for displaying solver graphs
## Used by floating screen to show spacecraft resource networks

@onready var graph_view: GraphEdit = $GraphView

func _ready():
	if not graph_view:
		graph_view = GraphEdit.new()
		graph_view.name = "GraphView"
		add_child(graph_view)
		
		# Configure graph view
		graph_view.anchors_preset = Control.PRESET_FULL_RECT
		graph_view.offset_left = 0
		graph_view.offset_top = 0
		graph_view.offset_right = 0
		graph_view.offset_bottom = 0

## Load and display a solver graph
func set_graph(graph: LCSolverGraph):
	if not graph_view:
		push_warning("SimpleGraphViewer: graph_view not ready")
		return
	
	# Load the graph using GraphView's method
	if graph_view.has_method("load_from_solver_graph"):
		graph_view.load_from_solver_graph(graph)
	else:
		# Fallback: manually create nodes
		_load_graph_manually(graph)

## Fallback method to load graph if GraphView doesn't have the method
func _load_graph_manually(graph: LCSolverGraph):
	const MODULE_PATH = "res://apps/supply_chain_modeling"
	
	graph_view.clear_connections()
	for child in graph_view.get_children():
		if child is GraphNode:
			child.queue_free()
	
	var node_map = {}
	var i = 0
	var spacing = 250
	var cols = 4
	
	# Create nodes
	for node_id in graph.nodes:
		var solver_node: LCSolverNode = graph.nodes[node_id]
		var ui_node_scene = load(MODULE_PATH + "/ui/widgets/ui_solver_node.tscn")
		if not ui_node_scene:
			push_warning("SimpleGraphViewer: Could not load ui_solver_node.tscn")
			continue
			
		var ui_node = ui_node_scene.instantiate()
		ui_node.solver_node = solver_node
		ui_node.name = "SolverNode_" + str(node_id)
		ui_node.position_offset = Vector2(100 + (i % cols) * spacing, 100 + (i / cols) * spacing)
		node_map[node_id] = ui_node.name
		graph_view.add_child(ui_node)
		i += 1
	
	# Create connections
	for edge_id in graph.edges:
		var edge: LCSolverEdge = graph.edges[edge_id]
		var from_name = node_map.get(edge.node_a.id)
		var to_name = node_map.get(edge.node_b.id)
		if from_name and to_name:
			graph_view.connect_node(from_name, 0, to_name, 0)
