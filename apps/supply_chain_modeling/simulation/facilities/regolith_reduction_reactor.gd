class_name RegolithReductionReactor
extends SolverSimulationNode

const SolverDomain = preload("res://core/systems/solver/solver_domain.gd")

# Input/output rates
@export var regolith_input_rate: float = 10.0  # kg/minute
@export var h2_input_rate: float = 1.0  # kg/minute
@export var power_consumption: float = 50.0  # kW
@export var water_output_rate: float = 5.0  # kg/minute
@export var waste_output_rate: float = 6.0  # kg/minute

# Internal buffers (optional, for smoothing)
var current_regolith: float = 0.0
var current_h2: float = 0.0

func _init():
	facility_type = "regolith_reactor"
	description = "Reduces Regolith with Hydrogen to produce Water"

func _create_ports():
	# Inputs
	ports["regolith_in"] = solver_graph.add_node(0.0, false, SolverDomain.SOLID)
	ports["h2_in"] = solver_graph.add_node(0.0, false, SolverDomain.GAS)
	ports["power_in"] = solver_graph.add_node(0.0, false, SolverDomain.ELECTRICAL)
	
	# Outputs
	ports["water_out"] = solver_graph.add_node(0.0, false, SolverDomain.GAS)
	ports["waste_out"] = solver_graph.add_node(0.0, false, SolverDomain.SOLID)

func _create_internal_edges():
	# No direct pass-through edges. 
	# Flows are driven by the reactor logic via flow sources on ports.
	pass

func update_solver_state():
	# Reset flow sources
	ports["regolith_in"].flow_source = 0.0
	ports["h2_in"].flow_source = 0.0
	ports["power_in"].flow_source = 0.0
	ports["water_out"].flow_source = 0.0
	ports["waste_out"].flow_source = 0.0
	
	# Check power availability (Electrical domain)
	# We act as a load (resistor) or current sink
	# Simplified: We demand current if potential is present
	var voltage = ports["power_in"].potential
	var power_available = 0.0
	
	if voltage > 0.1:
		# P = V * I -> I = P / V
		var current_demand = (power_consumption * 1000.0) / voltage # Amps
		ports["power_in"].flow_source = -current_demand # Consuming current
		power_available = power_consumption # Assume we get what we ask for (simplified)
	
	# If powered, drive the reaction
	if power_available >= power_consumption * 0.9:
		status = "Running"
		
		# Consume inputs (Negative flow source = consumption)
		# Convert kg/min to kg/s
		var regolith_rate_sec = regolith_input_rate / 60.0
		var h2_rate_sec = h2_input_rate / 60.0
		
		ports["regolith_in"].flow_source = -regolith_rate_sec
		ports["h2_in"].flow_source = -h2_rate_sec
		
		# Produce outputs (Positive flow source = production)
		var water_rate_sec = water_output_rate / 60.0
		var waste_rate_sec = waste_output_rate / 60.0
		
		ports["water_out"].flow_source = water_rate_sec
		ports["waste_out"].flow_source = waste_rate_sec
		
	else:
		status = "Insufficient Power"

func update_from_solver():
	# Update internal state or visualization if needed
	pass
