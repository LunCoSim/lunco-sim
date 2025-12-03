class_name LCSolverNode
extends RefCounted

## Node in the Linear Graph Solver
## Represents a point of potential (Pressure/Voltage) in a specific domain.
## Analogous to an Electrical Node or Modelica Port.

# Unique ID for the solver
var id: int = -1

# Domain of this node (e.g., "Fluid", "Electrical", "Thermal")
var domain: StringName = "Fluid"

# State Variable: Potential (Generic)
# Fluid: Pressure (Pascals)
# Electrical: Voltage (Volts)
# Thermal: Temperature (Kelvin)
var potential: float = 0.0

# Integration Variable: Flow Accumulation (Generic)
# Fluid: Mass (kg)
# Electrical: Charge (Coulombs)
# Thermal: Energy (Joules)
var flow_accumulation: float = 0.0

# Capacitance (Capacity to store flow accumulation per unit potential)
# Fluid: Area/Gravity (m^2/g) or Volume/BulkModulus (V/K) -> dMass/dPressure
# Electrical: Capacitance (Farads) -> dCharge/dVoltage
# Thermal: Heat Capacity (J/K) -> dEnergy/dTemperature
var capacitance: float = 0.0

# If true, this node is a storage node (Capacitance > 0)
# Its potential is determined by integration of flow.
var is_storage: bool = false

# If true, this node is a reference node (Ground/Atmosphere)
# Its potential is fixed and will not be solved for.
var is_ground: bool = false

# Resource Type (Optional, for Fluid domain)
# Defines what material is present/flowing (e.g., "oxygen", "water")
var resource_type: StringName = ""

# Connected edges (for graph traversal if needed)
var edges: Array[LCSolverEdge] = []

func _init(p_id: int, p_initial_potential: float = 0.0, p_is_ground: bool = false, p_domain: StringName = "Fluid"):
	id = p_id
	potential = p_initial_potential
	is_ground = p_is_ground
	domain = p_domain

func set_capacitance(p_capacitance: float):
	capacitance = p_capacitance
	is_storage = capacitance > 0.0

func add_edge(edge: LCSolverEdge):
	if not edge in edges:
		edges.append(edge)

func remove_edge(edge: LCSolverEdge):
	edges.erase(edge)
