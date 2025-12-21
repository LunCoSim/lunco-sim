class_name WaterCollectionSystem
extends SolverSimulationNode


# Rates
@export var condensation_rate: float = 5.0 # kg/minute
@export var power_consumption: float = 10.0 # kW

func _init():
	facility_type = "water_collection"
	description = "Condenses water vapor into liquid water"

func _create_ports():
	ports["vapor_in"] = solver_graph.add_node(0.0, false, SolverDomain.GAS)
	ports["water_out"] = solver_graph.add_node(0.0, false, SolverDomain.LIQUID)
	ports["power_in"] = solver_graph.add_node(0.0, false, SolverDomain.ELECTRICAL)

func _create_internal_edges():
	# We could model this as a direct connection with a "phase change" conductance
	# But for now, we'll drive it via flow sources to enforce the rate limit and power requirement
	pass

func update_solver_state():
	# Reset
	ports["vapor_in"].flow_source = 0.0
	ports["water_out"].flow_source = 0.0
	ports["power_in"].flow_source = 0.0
	
	# Power check
	var voltage = ports["power_in"].potential
	if voltage > 0.1:
		var current = (power_consumption * 1000.0) / voltage
		ports["power_in"].flow_source = -current
		
		status = "Running"
		
		# Transfer mass from Vapor to Water
		var rate_sec = condensation_rate / 60.0
		
		# Consume vapor
		ports["vapor_in"].flow_source = -rate_sec
		# Produce water
		ports["water_out"].flow_source = rate_sec
		
	else:
		status = "Insufficient Power"

func update_from_solver():
	pass
