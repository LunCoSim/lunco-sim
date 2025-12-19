class_name LCPumpComponent
extends LCResourceComponent

## Pump Component
## Physics: Voltage Source
## Adds pressure head to the flow path.
## Creates an intermediate pump node for visibility in graph viewer.

# Parameters
var max_pressure: float = 100000.0 # Pa (Head)
var max_flow: float = 10.0 # kg/s (Not strictly enforced by linear model, but implies resistance)
var power: float = 0.0 # 0.0 to 1.0 (Throttle) - starts closed

# Internal
var conductance: float = 1.0
var pump_node: LCSolverNode  # Intermediate node representing the pump
var inlet_edge: LCSolverEdge  # Source → Pump
var outlet_edge: LCSolverEdge  # Pump → Sink

func _init(p_graph: LCSolverGraph, p_max_pressure: float = 100000.0):
	super._init(p_graph)
	max_pressure = p_max_pressure
	
	# Calculate conductance from max_flow and max_pressure
	# This ensures pumps with different max_flow have different flow resistance
	# conductance = max_flow / max_pressure (kg/s per Pa)
	# Will be recalculated when max_flow is set
	_update_conductance()
	
	# Create intermediate pump node (Liquid domain, not storage, not ground)
	pump_node = graph.add_node(0.0, false, "Liquid")
	pump_node.display_name = "Pump"
	pump_node.resource_type = "pump"

func _update_conductance():
	# Conductance determines flow resistance
	# Higher max_flow → higher conductance → more flow
	if max_pressure > 0:
		conductance = max_flow / max_pressure
	else:
		conductance = 0.001  # Fallback

## Connect two existing nodes with this pump
func connect_nodes(node_in: LCSolverNode, node_out: LCSolverNode):
	if inlet_edge:
		graph.remove_edge(inlet_edge)
	if outlet_edge:
		graph.remove_edge(outlet_edge)
		
	# Create two edges: Source → Pump → Sink
	# Inlet: passive connection (no pressure source)
	inlet_edge = graph.connect_nodes(node_in, pump_node, conductance, "Liquid")
	
	# Outlet: active connection (pressure source applied here)
	outlet_edge = graph.connect_nodes(pump_node, node_out, conductance, "Liquid")

func set_power(p_power: float):
	power = clamp(p_power, 0.0, 1.0)

func update(delta: float):
	if outlet_edge and inlet_edge:
		# CRITICAL: Only pump if source has fluid (potential > 0)
		# Prevents pumping from empty tanks
		# Check the node BEFORE the pump (inlet_edge.node_a)
		var source_has_fluid = inlet_edge.node_a.potential > 1.0  # 1 Pa threshold
		
		if source_has_fluid and power > 0.01:
			# Apply pressure source on the outlet edge
			outlet_edge.potential_source = max_pressure * power
			outlet_edge.conductance = conductance
			inlet_edge.conductance = conductance
			
			# Flow limiting: Reduce conductance if flow exceeds max_flow
			# This simulates pump performance curve
			if abs(outlet_edge.flow_rate) > max_flow:
				var pressure_diff = abs(pump_node.potential - outlet_edge.node_b.potential + outlet_edge.potential_source)
				if pressure_diff > 0.1:
					# Adjust conductance to limit flow
					outlet_edge.conductance = max_flow / pressure_diff
		else:
			# No fluid or pump off - close valve
			outlet_edge.potential_source = 0.0
			outlet_edge.conductance = 0.0
			inlet_edge.conductance = 0.0
