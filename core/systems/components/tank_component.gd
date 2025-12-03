class_name LCTankComponent
extends LCResourceComponent

## Tank Component
## Physics: Capacitor
## Pressure = P_gas + rho * g * height
## Mass_dot = sum(flows)

# Parameters
var volume: float = 1.0 # m^3
var height: float = 1.0 # m
var base_area: float = 1.0 # m^2 (derived or set)
var density: float = 1000.0 # kg/m^3 (Water/Fuel)
var p_gas: float = 101325.0 # Pa (1 atm)
var gravity: float = 9.81 # m/s^2

# State
var mass: float = 0.0 # kg
var level: float = 0.0 # m (0.0 to height)

func _init(p_graph: LCSolverGraph, p_volume: float = 1.0, p_height: float = 1.0, p_density: float = 1000.0):
	super._init(p_graph)
	volume = p_volume
	height = p_height
	density = p_density
	base_area = volume / height
	
	# Create the single port at the bottom of the tank
	# Tank nodes are "ground" nodes - they enforce their pressure
	var port = _create_port("port", p_gas)
	port.is_ground = true

func set_initial_mass(p_mass: float):
	mass = clamp(p_mass, 0.0, volume * density)
	_update_level_and_pressure()

func update(delta: float):
	var port = ports["port"]
	
	# 1. Integrate Mass
	# Flow is defined as OUT of the node (positive = leaving tank)
	# So we subtract flow from mass.
	# Wait, LCSolverEdge flow is A->B.
	# We need to sum flows *into* the node.
	
	var net_flow_in = 0.0
	for edge in port.edges:
		if edge.node_b == port:
			net_flow_in += edge.flow_rate # Flow A->B (Into Port)
		else:
			net_flow_in -= edge.flow_rate # Flow B->A (Out of Port)
			
	mass += net_flow_in * delta
	mass = clamp(mass, 0.0, volume * density)
	
	# 2. Update Level and Pressure
	_update_level_and_pressure()

func _update_level_and_pressure():
	level = mass / (density * base_area)
	
	# Hydrostatic pressure at the bottom
	# P = P_gas + rho * g * h
	var hydrostatic_p = density * gravity * level
	var total_p = p_gas + hydrostatic_p
	
	# Update the solver node's pressure
	# Since this is a "ground" node, the solver won't override this value
	ports["port"].pressure = total_p
