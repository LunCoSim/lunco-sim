extends Control

## Lightweight graph viewer for displaying solver graphs
## Used by floating screen to show spacecraft resource networks

@onready var graph_view: GraphEdit = $GraphView
var details_panel # NodeDetailsPanel

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
	
	# Connect selection signals
	if not graph_view.node_selected.is_connected(_on_node_selected):
		graph_view.node_selected.connect(_on_node_selected)
	
	# Create details panel
	var panel_scene = load("res://apps/supply_chain_modeling/ui/node_details_panel.tscn")
	if panel_scene:
		details_panel = panel_scene.instantiate()
		add_child(details_panel)
		
		# Position top-right with proper anchors
		details_panel.anchor_left = 1.0  # Right edge
		details_panel.anchor_top = 0.0   # Top edge
		details_panel.anchor_right = 1.0
		details_panel.anchor_bottom = 0.0
		details_panel.offset_left = -270  # 270px from right edge
		details_panel.offset_top = 20     # 20px from top
		details_panel.offset_right = -20  # 20px margin from right
		details_panel.offset_bottom = 320 # Fixed height
		details_panel.grow_horizontal = Control.GROW_DIRECTION_BEGIN
		details_panel.grow_vertical = Control.GROW_DIRECTION_END
		details_panel.custom_minimum_size = Vector2(250, 200)
		details_panel.z_index = 100  # Ensure it's on top
		
		print("SimpleGraphViewer: Created details panel at position: ", details_panel.position)

	# Create search bar
	var search_bar = LineEdit.new()
	search_bar.placeholder_text = "Search nodes (ID, Domain)..."
	add_child(search_bar)
	search_bar.set_anchors_preset(Control.PRESET_TOP_LEFT)
	search_bar.position = Vector2(20, 20)
	search_bar.custom_minimum_size.x = 200
	search_bar.text_submitted.connect(_on_search_submitted)

func _on_node_selected(node):
	print("SimpleGraphViewer: Node selected: ", node.name if node else "null")
	if node and node.get("solver_node"):
		print("SimpleGraphViewer: Has solver_node, display_name: ", node.solver_node.display_name)
		if details_panel:
			details_panel.display_node(node.solver_node)
		else:
			print("SimpleGraphViewer: No details_panel!")
	else:
		print("SimpleGraphViewer: Node has no solver_node property")

func _on_search_submitted(text):
	if text.is_empty(): return
	
	# Find node with matching name or ID
	for child in graph_view.get_children():
		if child is GraphNode and child.get("solver_node"):
			var node = child.solver_node
			# Search by ID, Domain, or Resource Type
			if text in str(node.id) or text.to_lower() in str(node.domain).to_lower() or (node.resource_type and text.to_lower() in node.resource_type.to_lower()):
				# Found match
				child.selected = true
				graph_view.scroll_offset = child.position_offset - graph_view.size / 2
				_on_node_selected(child)
				break


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
