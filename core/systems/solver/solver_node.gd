class_name LCSolverNode
extends RefCounted

const SolverDomain = preload("res://core/systems/solver/solver_domain.gd")

## Node in the Linear Graph Solver
## Represents a point of potential (Pressure/Voltage) in a specific domain.
## Analogous to an Electrical Node or Modelica Port.

# Unique ID for the solver
var id: int = -1

# Domain of this node
var domain: StringName = SolverDomain.LIQUID

# State Variable: Potential (Generic)
# Liquid: Pressure (Pascals)
# Gas: Pressure (Pascals)
# Solid: Level/Index (Dimensionless or Meters)
# Electrical: Voltage (Volts)
# Thermal: Temperature (Kelvin)
var potential: float = 0.0

# Integration Variable: Flow Accumulation (Generic)
# Liquid/Gas/Solid: Mass (kg)
# Electrical: Charge (Coulombs)
# Thermal: Energy (Joules)
var flow_accumulation: float = 0.0

# Capacitance (Capacity to store flow accumulation per unit potential)
# Liquid: Area/Gravity (m^2/g) or Volume/BulkModulus (V/K) -> dMass/dPressure
# Gas: dMass/dPressure (derived from Gas Law)
# Solid: Area * Density -> dMass/dLevel
# Electrical: Capacitance (Farads) -> dCharge/dVoltage
# Thermal: Heat Capacity (J/K) -> dEnergy/dTemperature
var capacitance: float = 0.0

# If true, this node is a storage node (Capacitance > 0)
# Its potential is determined by integration of flow.
var is_storage: bool = false

# If true, this node is a reference node (Ground/Atmosphere)
# Its potential is fixed and will not be solved for.
var is_ground: bool = false

# Resource Type (Optional, for Liquid/Gas/Solid domain)
# Defines what material is present/flowing (e.g., "oxygen", "water", "regolith")
var resource_type: StringName = ""

# Display Name (for visualization)
var display_name: String = ""

# Effector Reference (for UI parameter control)
# Weak reference to the associated effector/component
var effector_ref: WeakRef = null

# Flow Source (External flow injection)
# Positive = Flow entering the node
# Liquid/Gas/Solid: Mass Flow In (kg/s)
# Electrical: Current Source (Amps)
var flow_source: float = 0.0

# Connected edges
var edges: Array = []

func _init(p_id: int, p_initial_potential: float = 0.0, p_is_ground: bool = false, p_domain: StringName = SolverDomain.LIQUID):
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
