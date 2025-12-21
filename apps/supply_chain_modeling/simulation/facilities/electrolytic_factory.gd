class_name ElectrolyticFactory
extends SolverSimulationNode


# Input/output rates
@export var h2o_input_rate: float = 2.0  # units/minute
@export var power_input_rate: float = 100.0  # kW
@export var h2_output_rate: float = 2.0  # units/minute
@export var o2_output_rate: float = 1.0  # units/minute
@export var power_consumption: float = 100.0  # kW

# Current resource amounts
@export var h2o_stored: float = 0.0
@export var power_available: float = 0.0

func _init():
	facility_type = "producer"
	description = "Breaks down H2O into H2 and O2 through electrolysis"

## Create ports for water input and gas outputs
func _create_ports():
	# Water inlet (Liquid)
	var water_in = solver_graph.add_node(0.0, false, SolverDomain.LIQUID)
	water_in.resource_type = "water"
	ports["water_in"] = water_in
	
	# H2 outlet (Gas)
	var h2_out = solver_graph.add_node(0.0, false, SolverDomain.GAS)
	h2_out.resource_type = "hydrogen"
	ports["h2_out"] = h2_out
	
	# O2 outlet (Gas)
	var o2_out = solver_graph.add_node(0.0, false, SolverDomain.GAS)
	o2_out.resource_type = "oxygen"
	ports["o2_out"] = o2_out
	
	# Power inlet (Electrical)
	ports["power_in"] = solver_graph.add_node(0.0, false, SolverDomain.ELECTRICAL)
	
	# Internal storage for buffering (small capacitance)
	var internal_buffer = solver_graph.add_node(0.0, false, SolverDomain.LIQUID)
	internal_buffer.set_capacitance(50.0)  # Larger buffer for stability
	internal_buffer.resource_type = "water"
	ports["_internal_buffer"] = internal_buffer

## Create internal edges
func _create_internal_edges():
	# Intake edge (water consumption)
	# Use connect_nodes helper which handles domain matching or warnings
	var intake_edge = solver_graph.connect_nodes(ports["water_in"], ports["_internal_buffer"], 1.0, SolverDomain.LIQUID)
	# intake_edge.is_unidirectional = true # Removed to prevent flow blocking issues
	internal_edges.append(intake_edge)
	
	# H2 production edge (from buffer to h2_out)
	var h2_edge = solver_graph.connect_nodes(ports["_internal_buffer"], ports["h2_out"], 0.1, SolverDomain.LIQUID)
	h2_edge.is_unidirectional = true
	internal_edges.append(h2_edge)
	
	var o2_edge = solver_graph.connect_nodes(ports["_internal_buffer"], ports["o2_out"], 0.1, SolverDomain.LIQUID)
	o2_edge.is_unidirectional = true
	internal_edges.append(o2_edge)

## Update solver parameters from component state
func update_solver_state():
	# Check power
	ports["power_in"].flow_source = 0.0
	power_available = 0.0
	
	var voltage = ports["power_in"].potential
	if voltage > 0.1:
		var current_demand = (power_consumption * 1000.0) / voltage
		ports["power_in"].flow_source = -current_demand
		power_available = power_consumption # Simplified
	
	if internal_edges.size() < 3:
		return
	
	# Calculate production rate based on power availability
	var power_ratio = 1.0
	if power_consumption > 0:
		power_ratio = clamp(power_available / power_consumption, 0.0, 1.0)
	
	var effective_efficiency = efficiency * power_ratio
	
	# Intake edge (water consumption)
	var intake_edge: LCSolverEdge = internal_edges[0]
	# Allow free flow into the buffer (high conductance) when valves are open
	intake_edge.conductance = 5.0 * effective_efficiency
	
	# H2 production edge
	var h2_edge: LCSolverEdge = internal_edges[1]
	# Drive flow with potential source (pressure pump effect)
	h2_edge.potential_source = (h2_output_rate / 60.0) * effective_efficiency * 10.0 
	h2_edge.conductance = (h2_output_rate / 60.0) * effective_efficiency
	
	# O2 production edge
	var o2_edge: LCSolverEdge = internal_edges[2]
	o2_edge.potential_source = (o2_output_rate / 60.0) * effective_efficiency * 10.0
	o2_edge.conductance = (o2_output_rate / 60.0) * effective_efficiency

## Update component state from solver results
func update_from_solver():
	if internal_edges.size() < 3:
		status = "Not Connected"
		return
	
	# Check power
	if power_available < power_consumption * 0.1:
		status = "Insufficient Power"
		return
	
	# Check if we're producing
	var h2_edge: LCSolverEdge = internal_edges[1]
	var o2_edge: LCSolverEdge = internal_edges[2]
	
	if h2_edge.flow_rate > 0.001 or o2_edge.flow_rate > 0.001:
		status = "Running"
	elif ports.has("_internal_buffer"):
		var buffer: LCSolverNode = ports["_internal_buffer"]
		# Debug buffer status
		if buffer.flow_accumulation < 0.001: # Lowered threshold
			status = "Insufficient H2O"
			if Engine.get_process_frames() % 60 == 0:
				print("Factory [%s]: H2O Low! Buffer=%.6f, IntakeFlow=%.6f" % [name, buffer.flow_accumulation, internal_edges[0].flow_rate])
		else:
			status = "Output Storage Full"
	else:
		status = "Idle"

func save_state() -> Dictionary:
	var state = super.save_state()
	state["h2o_input_rate"] = h2o_input_rate
	state["power_input_rate"] = power_input_rate
	state["h2_output_rate"] = h2_output_rate
	state["o2_output_rate"] = o2_output_rate
	state["power_consumption"] = power_consumption
	return state

func load_state(state: Dictionary) -> void:
	super.load_state(state)
	h2o_input_rate = state.get("h2o_input_rate", h2o_input_rate)
	power_input_rate = state.get("power_input_rate", power_input_rate)
	h2_output_rate = state.get("h2_output_rate", h2_output_rate)
	o2_output_rate = state.get("o2_output_rate", o2_output_rate)
	power_consumption = state.get("power_consumption", power_consumption)
