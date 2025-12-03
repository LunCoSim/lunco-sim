class_name LCSolverGraph
extends RefCounted

## Linear Graph Solver Engine
## Solves for Pressure and Flow in a network of nodes and edges.
## Uses an iterative approach (Successive Over-Relaxation) suitable for real-time.

# Graph Topology
var nodes: Dictionary = {} # id -> LCSolverNode
var edges: Dictionary = {} # id -> LCSolverEdge
var _next_node_id: int = 0
var _next_edge_id: int = 0

# Solver Settings
var max_iterations: int = 10
var relaxation: float = 1.5 # Over-relaxation factor (1.0 = Gauss-Seidel)
var tolerance: float = 0.001

# --- Topology Management ---

func add_node(initial_pressure: float = 0.0, is_ground: bool = false) -> LCSolverNode:
	var node = LCSolverNode.new(_next_node_id, initial_pressure, is_ground)
	nodes[_next_node_id] = node
	_next_node_id += 1
	return node

func connect_nodes(node_a: LCSolverNode, node_b: LCSolverNode, conductance: float) -> LCSolverEdge:
	if not node_a or not node_b:
		push_error("Cannot connect null nodes")
		return null
		
	var edge = LCSolverEdge.new(_next_edge_id, node_a, node_b, conductance)
	edges[_next_edge_id] = edge
	_next_edge_id += 1
	
	node_a.add_edge(edge)
	node_b.add_edge(edge)
	
	return edge

func remove_node(node: LCSolverNode):
	if not node: return
	
	# Remove all connected edges first
	# We iterate a copy because we'll be modifying the array
	var connected_edges = node.edges.duplicate()
	for edge in connected_edges:
		remove_edge(edge)
	
	nodes.erase(node.id)

func remove_edge(edge: LCSolverEdge):
	if not edge: return
	
	edge.node_a.remove_edge(edge)
	edge.node_b.remove_edge(edge)
	edges.erase(edge.id)

# --- Solver Core ---

## Main solve step. Call this every physics frame.
## 1. Solve for Pressures (Iterative)
## 2. Update Flows
func solve(delta: float):
	_solve_pressures()
	_update_flows()

## Iteratively solve for node pressures to satisfy Kirchhoff's Current Law
## sum(Flow_in) = 0  =>  sum(G * (P_neighbor - P_node)) = 0
func _solve_pressures():
	for i in range(max_iterations):
		var max_error = 0.0
		
		for node_id in nodes:
			var node: LCSolverNode = nodes[node_id]
			
			# Skip ground nodes (fixed pressure)
			if node.is_ground:
				continue
				
			# Calculate target pressure based on neighbors
			# P_node = sum(G_i * P_i) / sum(G_i)
			var sum_g_p = 0.0
			var sum_g = 0.0
			
			for edge in node.edges:
				var neighbor = edge.get_other_node(node)
				var g = edge.conductance
				
				# Kirchhoff's Law: sum(Flow_in) = 0
				# Flow_in = G * (P_neighbor - P_node + P_source_towards_node)
				
				# Determine direction of pressure source
				# If edge is A->B and we are B, source adds to flow in.
				# If edge is A->B and we are A, source subtracts from flow in (adds to flow out).
				
				var p_source_effect = 0.0
				if node == edge.node_b:
					p_source_effect = edge.pressure_source
				else:
					p_source_effect = -edge.pressure_source
				
				# Contribution to sum_g_p: G * (P_neighbor + P_source_effect)
				sum_g_p += g * (neighbor.pressure + p_source_effect)
				sum_g += g
			
			if sum_g > 0.000001:
				var target_pressure = sum_g_p / sum_g
				
				# Apply relaxation
				# P_new = P_old + w * (P_target - P_old)
				var new_pressure = node.pressure + relaxation * (target_pressure - node.pressure)
				
				# Track convergence
				var error = abs(new_pressure - node.pressure)
				if error > max_error:
					max_error = error
					
				node.pressure = new_pressure
		
		# Early exit if converged
		if max_error < tolerance:
			break

## Update flow rates on all edges based on final pressures
func _update_flows():
	for edge_id in edges:
		var edge: LCSolverEdge = edges[edge_id]
		edge.update_flow()
