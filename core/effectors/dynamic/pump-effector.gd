class_name LCPumpEffector
extends LCDynamicEffector

## Pump dynamic effector that controls propellant flow.
##
## Provides user-controllable valve between a source (tank) and sink (engine).
## Uses LCPumpComponent to add pressure head and modulate flow via pump power.

@export_group("Pump Properties")
@export var max_pressure: float = 500000.0  ## Maximum pressure head in Pa (5 bar typical turbopump)
@export var max_flow: float = 50.0  ## Maximum flow rate in kg/s
@export var pump_efficiency: float = 0.85  ## Pump efficiency (0.0 to 1.0)
@export var action_channel: String = "thrust" ## Action this pump responds to

@export_group("Connections")
@export var source_path: NodePath  ## Source node (typically a tank)
@export var sink_path: NodePath  ## Sink node (typically an engine or consumer)

# Component references
var source_node: Node = null
var sink_node: Node = null
var component: LCPumpComponent = null

# Solver Integration
var solver_graph: LCSolverGraph

# Control inputs
var pump_power: float = 0.0  ## User-set pump power (0.0 to 1.0)

# Internal state
var actual_flow_rate: float = 0.0  ## Current flow rate in kg/s
var pressure_head: float = 0.0  ## Current pressure differential in Pa
var is_active: bool = false

func _init():
	_initialize_parameters()

func _initialize_parameters():
	# User-controllable pump power
	Parameters["Pump Power %"] = { "path": "pump_power", "type": "float", "min": 0.0, "max": 1.0, "step": 0.01, "category": "control" }
	
	# Read-only status displays
	Parameters["Flow Rate (kg/s)"] = { "path": "actual_flow_rate", "type": "float", "readonly": true, "category": "status" }
	Parameters["Pressure Head (kPa)"] = { "path": "pressure_head_kpa", "type": "float", "readonly": true, "category": "status" }
	Parameters["Active"] = { "path": "is_active", "type": "bool", "readonly": true, "category": "status" }

func _ready():
	super._ready()
	
	# Connect to source and sink nodes
	if not source_path.is_empty():
		source_node = get_node_or_null(source_path)
		if source_node:
			print("✓ LCPumpEffector: Connected to source ", source_node.name)
		else:
			print("✗ LCPumpEffector: Invalid source path: ", source_path)
	
	if not sink_path.is_empty():
		sink_node = get_node_or_null(sink_path)
		if sink_node:
			print("✓ LCPumpEffector: Connected to sink ", sink_node.name)
		else:
			print("✗ LCPumpEffector: Invalid sink path: ", sink_path)
	
	# Set power consumption (rough estimate: 1kW per 10kg/s at 5 bar)
	power_consumption = max_flow * 0.1
	
	_initialize_telemetry()

## Set the solver graph (called by vehicle during _ready)
func set_solver_graph(graph: LCSolverGraph):
	solver_graph = graph
	if solver_graph and source_node and sink_node:
		# Create pump component with configured max_flow
		component = LCPumpComponent.new(solver_graph, max_pressure)
		component.max_flow = max_flow  # CRITICAL: Set max_flow from exported property
		component._update_conductance()  # Recalculate conductance based on max_flow
		
		# Set display name and link effector to pump node
		component.pump_node.display_name = name
		component.pump_node.effector_ref = weakref(self)
		
		# Defer connection to next frame to ensure all solver nodes exist
		# (Engine solver nodes are created in deferred _initialize_solver_graph)
		call_deferred("_connect_to_solver_nodes")

## Connect to solver nodes (called deferred to ensure nodes exist)
func _connect_to_solver_nodes():
	if not component or not source_node or not sink_node:
		return
	
	# Get ports from source and sink
	var source_port = null
	var sink_port = null
	
	# Source is typically a tank with get_port() method
	if source_node.has_method("get_port"):
		source_port = source_node.get_port()
	
	# Sink could be an engine's solver_node or another component
	if sink_node.has_method("get_port"):
		sink_port = sink_node.get_port()
	elif "solver_node" in sink_node and sink_node.solver_node:
		sink_port = sink_node.solver_node
	
	if source_port and sink_port:
		component.connect_nodes(source_port, sink_port)
		print("✓ LCPumpEffector: Connected %s → %s in solver graph (Liquid domain)" % [source_node.name, sink_node.name])
	else:
		print("✗ LCPumpEffector: Failed to get solver ports")
		if not source_port:
			print("  Source port missing from ", source_node.name)
		if not sink_port:
			print("  Sink port missing from ", sink_node.name)

func _physics_process(delta):
	_update_pump_power()
	if component:
		component.update(delta)
	_read_flow_state()

func _process(delta):
	_update_telemetry()

## Sets the pump power (0.0 to 1.0)
func set_pump_power(power: float):
	pump_power = clamp(power, 0.0, 1.0)

# --- Control Interface ---

func get_control_actions() -> Array[String]:
	return [action_channel]

func apply_control(action: String, value: float):
	if action == action_channel:
		set_pump_power(value)

## Updates pump component power based on user input
func _update_pump_power():
	if component:
		component.set_power(pump_power)
		is_active = pump_power > 0.01

## Reads current flow state from solver
func _read_flow_state():
	if component and component.outlet_edge:
		actual_flow_rate = abs(component.outlet_edge.flow_rate)
		pressure_head = component.outlet_edge.potential_source
	else:
		actual_flow_rate = 0.0
		pressure_head = 0.0

## Helper for UI display (pressure in kPa)
var pressure_head_kpa: float:
	get:
		return pressure_head / 1000.0

func _initialize_telemetry():
	Telemetry = {
		"pump_power": pump_power,
		"flow_rate": actual_flow_rate,
		"pressure_head": pressure_head,
		"is_active": is_active,
	}

func _update_telemetry():
	if not Telemetry:
		return
	
	Telemetry["pump_power"] = pump_power
	Telemetry["flow_rate"] = actual_flow_rate
	Telemetry["pressure_head"] = pressure_head
	Telemetry["is_active"] = is_active

# Command interface
func cmd_set_power(power: float):
	set_pump_power(power)

func cmd_on():
	set_pump_power(1.0)

func cmd_off():
	set_pump_power(0.0)
