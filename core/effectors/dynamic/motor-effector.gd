class_name LCMotorEffector
extends LCDynamicEffector

## Electric Motor Effector
##
## Converts electrical power to mechanical torque
## Consumes power from the electrical domain

@export_group("Motor Properties")
@export var max_torque: float = 50.0  ## Maximum torque in Nm
@export var max_rpm: float = 3000.0  ## Maximum RPM
@export var motor_efficiency: float = 0.85  ## Motor efficiency (0.0 to 1.0)
@export var nominal_voltage: float = 28.0  ## Nominal operating voltage

@export_group("Power")
@export var max_power_draw: float = 500.0  ## Maximum power consumption in Watts

# Internal state
var current_torque: float = 0.0  ## Current torque output in Nm
var current_rpm: float = 0.0  ## Current RPM
var current_power: float = 0.0  ## Current power consumption in Watts
var target_torque: float = 0.0  ## Commanded torque

# Solver Integration
var solver_graph: LCSolverGraph
var solver_node: LCSolverNode  ## Electrical node for power consumption

func _ready():
	super._ready()
	mass = 2.0  # Motor mass
	_initialize_telemetry()

## Set the solver graph (called by vehicle during _ready)
func set_solver_graph(graph: LCSolverGraph):
	solver_graph = graph
	if solver_graph and not solver_node:
		# Create electrical load node
		solver_node = solver_graph.add_node(0.0, false, "Electrical")
		solver_node.resource_type = "electrical_power"
		solver_node.display_name = name  # Use effector's name
		solver_node.effector_ref = weakref(self)
		
		print("Motor '%s': Created solver node as electrical load" % name)

func _physics_process(delta):
	_update_motor(delta)

func _process(delta):
	_update_telemetry()

## Set the desired torque output
func set_torque(torque: float):
	target_torque = clamp(torque, -max_torque, max_torque)

## Updates motor state and power consumption
func _update_motor(delta: float):
	# Simple motor model: torque proportional to command
	current_torque = target_torque
	
	# Calculate power consumption
	# P = T * ω / efficiency
	# ω (rad/s) = RPM * 2π / 60
	var omega = current_rpm * 2.0 * PI / 60.0
	var mechanical_power = abs(current_torque * omega)
	current_power = mechanical_power / motor_efficiency if motor_efficiency > 0 else 0.0
	current_power = min(current_power, max_power_draw)
	
	# Update power consumption for vehicle
	power_consumption = current_power
	
	# Update solver node (negative flow = consumption)
	if solver_node:
		# Current draw: I = P / V
		var voltage = solver_node.potential if solver_node.potential > 1.0 else nominal_voltage
		var current_draw = current_power / voltage
		solver_node.flow_source = -current_draw  # Negative = consumption

## Get current power consumption
func get_power_draw() -> float:
	return current_power

## Get current torque output
func get_torque() -> float:
	return current_torque

func _initialize_telemetry():
	Telemetry = {
		"torque": current_torque,
		"rpm": current_rpm,
		"power": current_power,
		"target_torque": target_torque,
	}

func _update_telemetry():
	Telemetry["torque"] = current_torque
	Telemetry["rpm"] = current_rpm
	Telemetry["power"] = current_power
	Telemetry["target_torque"] = target_torque
