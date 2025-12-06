class_name LCSolverGraph
extends RefCounted

const SolverDomain = preload("res://core/systems/solver/solver_domain.gd")

## Linear Graph Solver Engine
## Solves for Potential and Flow in a network of nodes and edges.
## Uses an iterative approach (Successive Over-Relaxation) suitable for real-time.
## Supports multiple domains (Fluid, Electrical, etc.) and dynamic storage (Capacitance).

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

func add_node(initial_potential: float = 0.0, is_ground: bool = false, domain: StringName = SolverDomain.LIQUID) -> LCSolverNode:
	var node = LCSolverNode.new(_next_node_id, initial_potential, is_ground, domain)
	nodes[_next_node_id] = node
	_next_node_id += 1
	return node

func connect_nodes(node_a: LCSolverNode, node_b: LCSolverNode, conductance: float, domain: StringName = SolverDomain.LIQUID) -> LCSolverEdge:
	if not node_a or not node_b:
		push_error("Cannot connect null nodes")
		return null
		
	if node_a.domain != domain or node_b.domain != domain:
		push_warning("Connecting nodes with mismatched domains to edge domain")
		
	var edge = LCSolverEdge.new(_next_edge_id, node_a, node_b, conductance, domain)
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
## 1. Solve for Potentials (Iterative KCL)
## 2. Update Flows
## 3. Integrate Storage (Mass/Charge Accumulation)
func solve(delta: float):
	_solve_potentials()
	_update_flows()
	_integrate_storage(delta)

## Iteratively solve for node potentials to satisfy Kirchhoff's Current Law
## sum(Flow_in) = 0  =>  sum(G * (P_neighbor - P_node)) = 0
## For Storage Nodes (C > 0), Potential is treated as fixed boundary condition during this step.
func _solve_potentials():
	for i in range(max_iterations):
		var max_error = 0.0
		
		for node_id in nodes:
			var node: LCSolverNode = nodes[node_id]
			
			# Skip ground nodes (fixed potential)
			if node.is_ground:
				continue
				
			# Skip storage nodes (potential determined by integration, fixed for KCL step)
			if node.is_storage:
				continue
				
			# Calculate target potential based on neighbors
			# KCL: sum(Flow_in) + Flow_source = 0
			# Flow_in = G * (P_neighbor - P_node + P_source_towards_node)
			# P_node = (sum(G * (P_neighbor + P_source_effect)) + Flow_source) / sum(G)
			var sum_g_p = 0.0
			var sum_g = 0.0
			
			for edge in node.edges:
				var neighbor = edge.get_other_node(node)
				var g = edge.conductance
				
				# Kirchhoff's Law: sum(Flow_in) = 0
				# Flow_in = G * (P_neighbor - P_node + P_source_towards_node)
				
				# Determine direction of potential source
				var p_source_effect = 0.0
				if node == edge.node_b:
					p_source_effect = edge.potential_source
				else:
					p_source_effect = -edge.potential_source
				
				# Contribution to sum_g_p: G * (P_neighbor + P_source_effect)
				sum_g_p += g * (neighbor.potential + p_source_effect)
				sum_g += g
			
			# Add flow source contribution
			# Flow_source is already in flow units (kg/s or Amps)
			sum_g_p += node.flow_source
			
			if sum_g > 0.000001:
				var target_potential = sum_g_p / sum_g
				
				# Apply relaxation
				# P_new = P_old + w * (P_target - P_old)
				var new_potential = node.potential + relaxation * (target_potential - node.potential)
				
				# Track convergence
				var error = abs(new_potential - node.potential)
				if error > max_error:
					max_error = error
					
				node.potential = new_potential
		
		# Early exit if converged
		if max_error < tolerance:
			break

## Update flow rates on all edges based on final potentials
func _update_flows():
	for edge_id in edges:
		var edge: LCSolverEdge = edges[edge_id]
		edge.update_flow()

## Integrate flow accumulation for storage nodes
func _integrate_storage(delta: float):
	for node_id in nodes:
		var node: LCSolverNode = nodes[node_id]
		
		if node.is_storage and not node.is_ground:
			var net_flow_in = 0.0
			
			for edge in node.edges:
				# Flow is defined positive A -> B
				if node == edge.node_b:
					net_flow_in += edge.flow_rate
				else:
					net_flow_in -= edge.flow_rate
			
			# Integrate
			node.flow_accumulation += net_flow_in * delta
			
			# Clamping (Mass cannot be negative)
			if node.flow_accumulation < 0.0:
				node.flow_accumulation = 0.0
			
			# Update Potential (Linear Model: P = Mass / Capacitance)
			# TODO: Support non-linear models (Gas Law) via strategy pattern or override
			if node.capacitance > 0.0:
				node.potential = node.flow_accumulation / node.capacitance
