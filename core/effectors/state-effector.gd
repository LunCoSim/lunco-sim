class_name LCStateEffector
extends LCComponent

## Base class for components that contribute state (mass, inertia, power) to a vehicle.
##
## These components are passive and provide property contributions when queried.

signal mass_changed


func _ready():
	super._ready()

## Returns the mass of this component in kg.
## Override this if mass is dynamic (e.g. fuel tank).
func get_mass_contribution() -> float:
	return mass

## Returns the inertia tensor contribution of this component.
## Currently returns zero (point mass approximation).
## Override for accurate physics.
func get_inertia_contribution() -> Vector3:
	return Vector3.ZERO

## Returns the center of mass offset relative to the vehicle origin.
## By default, uses the component's local position.
func get_center_of_mass_offset() -> Vector3:
	return position

## Returns power consumption in Watts.
func get_power_consumption() -> float:
	return power_consumption

## Returns power production in Watts.
func get_power_production() -> float:
	return power_production

# --- Solver Integration ---
var solver_graph: LCSolverGraph
var solver_node: LCSolverNode

## Set the solver graph (called by vehicle during _ready)
## Subclasses (like Battery) may override this.
func set_solver_graph(graph: LCSolverGraph):
	solver_graph = graph
	if solver_graph and not solver_node:
		# Only create a node if we consume or produce power
		if power_consumption > 0.0 or power_production > 0.0:
			# Create electrical node
			solver_node = solver_graph.add_node(0.0, false, "Electrical")
			solver_node.resource_type = "electrical_power"
			solver_node.display_name = name
			
			print("Effector '%s': Created solver node (Load: %.1f W)" % [name, power_consumption])

func _physics_process(delta):
	_update_solver_power()

func _update_solver_power():
	if solver_node:
		# Calculate net power flow (positive = production, negative = consumption)
		var net_power = power_production - power_consumption
		
		# Update flow source (Amps)
		# I = P / V
		var bus_voltage = 28.0 # Default fallback
		if solver_node.potential > 1.0:
			bus_voltage = solver_node.potential
			
		var current_flow = net_power / bus_voltage
		
		solver_node.flow_source = current_flow
		
		# Calculate actual power consumed/produced from solver state
		# P = V * I (where I is the flow into/out of the node)
		# For a load, flow is negative, so P is negative (consumption)
		# We want to expose the magnitude of power transfer
		var actual_power = bus_voltage * current_flow
		
		# Update telemetry with actual values
		if Telemetry:
			Telemetry["bus_voltage"] = bus_voltage
			Telemetry["current_draw"] = abs(current_flow)
			Telemetry["actual_power"] = abs(actual_power)
