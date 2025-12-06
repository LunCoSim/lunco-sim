class_name Pump
extends SolverSimulationNode

const SolverDomain = preload("res://core/systems/solver/solver_domain.gd")

# Pump properties
@export var pump_rate: float = 10.0  # units/minute
@export var power_consumption: float = 50.0  # kW
@export var power_available: float = 0.0
@export var domain: StringName = SolverDomain.LIQUID # Default to Liquid, can be changed to Gas

func _init():
	facility_type = "pump"
	description = "Pumps resources between storages"

## Create inlet and outlet ports
func _create_ports():
	# Inlet port (junction node - no storage)
	ports["inlet"] = solver_graph.add_node(0.0, false, domain)
	
	# Outlet port (junction node - no storage)
	ports["outlet"] = solver_graph.add_node(0.0, false, domain)
	
	# Power inlet (Electrical)
	ports["power_in"] = solver_graph.add_node(0.0, false, SolverDomain.ELECTRICAL)

## Create internal edge (the pump itself)
func _create_internal_edges():
	var pump_edge = solver_graph.connect_nodes(ports["inlet"], ports["outlet"], 1.0, domain)
	pump_edge.is_unidirectional = true  # Pumps only push one direction
	internal_edges.append(pump_edge)

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
	
	if internal_edges.size() == 0:
		return
		
	var pump_edge: LCSolverEdge = internal_edges[0]
	
	# Calculate effective pump pressure based on power availability
	# If we have enough power, apply full pump pressure
	# Otherwise, scale down proportionally
	var power_ratio = 1.0
	if power_consumption > 0:
		power_ratio = clamp(power_available / power_consumption, 0.0, 1.0)
	
	# Convert pump_rate (units/min) to pressure source
	# Pump pressure should be strong enough to overcome typical back-pressure
	# Using pump_rate directly as a pressure multiplier (simplified model)
	var target_pressure_source = pump_rate * efficiency * power_ratio * 10.0 # Multiplier for stronger pump effect
	
	# Pump pushes from Inlet to Outlet
	# potential_source adds to flow from A->B: Flow = G * (Pa - Pb + Psource)
	# So if A=Inlet, B=Outlet, Psource > 0 helps flow.
	pump_edge.potential_source = target_pressure_source
	
	# Update conductance (resistance to flow)
	# Higher pump rate = higher conductance when powered
	pump_edge.conductance = max(0.1, pump_rate * power_ratio)

## Update component state from solver results
func update_from_solver():
	# Update status based on flow
	if internal_edges.size() == 0:
		status = "Not Connected"
		return
		
	var pump_edge: LCSolverEdge = internal_edges[0]
	
	if power_available < power_consumption * 0.1:
		status = "Insufficient Power"
		if Engine.get_process_frames() % 60 == 0:
			print("Pump [%s]: Low Power! V=%.2f, Req=%.2f" % [name, ports["power_in"].potential, power_consumption])
	elif pump_edge.flow_rate > 0.01:
		status = "Running"
	else:
		status = "Idle"
		if Engine.get_process_frames() % 60 == 0:
			var inlet_edges = ports["inlet"].edges.size()
			var outlet_edges = ports["outlet"].edges.size()
			print("Pump [%s]: Idle. P_In=%.2f, P_Out=%.2f, Flow=%.4f. Edges: In=%d, Out=%d" % [name, ports["inlet"].potential, ports["outlet"].potential, pump_edge.flow_rate, inlet_edges, outlet_edges])
			
			if inlet_edges < 2: # 1 internal edge (pump) + 0 external
				print("Pump [%s]: WARNING - Inlet NOT connected to Source!" % name)

func save_state() -> Dictionary:
	var state = super.save_state()
	state["pump_rate"] = pump_rate
	state["power_consumption"] = power_consumption
	return state

func load_state(state: Dictionary) -> void:
	super.load_state(state)
	pump_rate = state.get("pump_rate", pump_rate)
	power_consumption = state.get("power_consumption", power_consumption)
