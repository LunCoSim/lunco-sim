class_name LCPipeComponent
extends LCResourceComponent

## Pipe Component
## Physics: Resistor
## Flow = Conductance * (P_in - P_out)

# Parameters
var length: float = 1.0 # m
var diameter: float = 0.1 # m
var roughness: float = 0.0001 # m (unused for simple linear model)
var viscosity: float = 0.001 # Pa*s (Water)

# Derived
var conductance: float = 1.0

# The edge in the solver graph
var edge: LCSolverEdge

func _init(p_graph: LCSolverGraph, p_length: float = 1.0, p_diameter: float = 0.1):
	super._init(p_graph)
	length = p_length
	diameter = p_diameter
	_calculate_conductance()

## Connect two existing nodes with this pipe
func connect_nodes(node_a: LCSolverNode, node_b: LCSolverNode):
	if edge:
		graph.remove_edge(edge)
		
	edge = graph.connect_nodes(node_a, node_b, conductance)

func _calculate_conductance():
	# Hagen-Poiseuille equation for laminar flow (simplified linear model)
	# R = (8 * mu * L) / (pi * r^4)
	# G = 1/R
	
	var radius = diameter / 2.0
	var resistance = (8.0 * viscosity * length) / (PI * pow(radius, 4))
	
	# Avoid division by zero
	if resistance < 0.000001:
		resistance = 0.000001
		
	conductance = 1.0 / resistance

func update(delta: float):
	# If we needed dynamic resistance (e.g. valve), we'd update conductance here
	if edge:
		edge.conductance = conductance
