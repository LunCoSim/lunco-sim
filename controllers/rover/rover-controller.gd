@icon("res://controllers/rover/rover.svg")
class_name LCRoverController
extends LCController

## Unified rover controller supporting multiple drive modes and individual wheel control
## Drive modes: Standard, Ackermann, Differential, Independent

# Export categories for easy configuration in the editor
@export_category("Drive Configuration")
@export_enum("Standard:0", "Ackermann:1", "Differential:2", "Independent:3") var drive_mode: int = 0
@export var enable_individual_control: bool = false

@export_category("Rover Movement Parameters")
@export var ENGINE_FORCE := 1200.0  # Reduced force to prevent flipping
@export var STEERING_FORCE := 0.6  # Increased for better steering response
@export var MAX_SPEED := 3.5  # Realistic max speed for lunar rover
@export var BRAKE_FORCE := 800.0  # Increased braking for better control

@export_category("Wheel References")
@export var front_left_wheel: NodePath
@export var front_right_wheel: NodePath
@export var back_left_wheel: NodePath
@export var back_right_wheel: NodePath

# Get the parent VehicleBody3D node
@onready var parent: VehicleBody3D:
	get:
		var p = self.get_parent()
		if p and p is VehicleBody3D:
			return p
		else:
			push_error("RoverController: Parent is not a VehicleBody3D! Got: " + str(p))
			return null

# Wheel references
var fl_wheel: LCWheelEffector
var fr_wheel: LCWheelEffector
var bl_wheel: LCWheelEffector
var br_wheel: LCWheelEffector

# Internal state
var motor_input := 0.0
var steering_input := 0.0
var crab_input := 0.0
var brake_input := 0.0
var current_speed := 0.0

# Previous values for change detection
var prev_motor_input := 0.0
var prev_steering_input := 0.0
var prev_speed := 0.0

# Slope compensation optimization
var slope_check_timer := 0.0
const SLOPE_CHECK_INTERVAL := 0.2  # Check slope every 200ms instead of every physics frame

# Individual wheel control (for Independent mode)
var wheel_controls := {
	"front_left": {"motor": 0.0, "brake": 0.0, "steering": 0.0},
	"front_right": {"motor": 0.0, "brake": 0.0, "steering": 0.0},
	"back_left": {"motor": 0.0, "brake": 0.0, "steering": 0.0},
	"back_right": {"motor": 0.0, "brake": 0.0, "steering": 0.0}
}

# Signals
signal motor_state_changed(power: float)
signal steering_changed(angle: float)
signal speed_changed(speed: float)
signal brake_applied(force: float)
signal wheel_control_changed(wheel_name: String, motor: float, brake: float, steering: float)

# Initialize the controller
func _ready():
	# Ensure we're in the right group for discovery
	if not is_in_group("RoverControllers"):
		add_to_group("RoverControllers")
	
	# Register Parameters
	Parameters["Engine Force"] = { "path": "ENGINE_FORCE", "type": "float", "min": 100.0, "max": 5000.0, "step": 100.0 }
	Parameters["Steering Force"] = { "path": "STEERING_FORCE", "type": "float", "min": 0.1, "max": 1.0, "step": 0.05 }
	Parameters["Brake Force"] = { "path": "BRAKE_FORCE", "type": "float", "min": 100.0, "max": 5000.0, "step": 100.0 }
	Parameters["Max Speed"] = { "path": "MAX_SPEED", "type": "float", "min": 1.0, "max": 20.0, "step": 0.5 }
	
	# Find wheel references
	_discover_wheels()
	
	# Reset inputs on start
	_reset_inputs()
	
	# Ensure parent is a VehicleBody3D
	if not parent is VehicleBody3D:
		push_error("RoverController's parent must be a VehicleBody3D")
	
	# Add command executor
	var executor = LCCommandExecutor.new()
	executor.name = "CommandExecutor"
	add_child(executor)
	print("RoverController: _ready complete. Created executor: ", executor, " path: ", executor.get_path())

func _discover_wheels():
	"""Automatically discover wheels if paths not set, or use explicit paths"""
	if front_left_wheel.is_empty():
		fl_wheel = _find_wheel_by_name("FrontLeftWheel")
	else:
		fl_wheel = get_node_or_null(front_left_wheel)
	
	if front_right_wheel.is_empty():
		fr_wheel = _find_wheel_by_name("FrontRightWheel")
	else:
		fr_wheel = get_node_or_null(front_right_wheel)
	
	if back_left_wheel.is_empty():
		bl_wheel = _find_wheel_by_name("BackLeftWheel")
	else:
		bl_wheel = get_node_or_null(back_left_wheel)
	
	if back_right_wheel.is_empty():
		br_wheel = _find_wheel_by_name("BackRightWheel")
	else:
		br_wheel = get_node_or_null(back_right_wheel)
	
	# Validate wheels were found
	if not fl_wheel or not fr_wheel or not bl_wheel or not br_wheel:
		if drive_mode != 0: # Standard mode uses VehicleBody3D built-in steering mostly
			push_warning("RoverController: Not all wheels found! Advanced drive modes may not work correctly.")
		print("  Wheels discovery: FL:", fl_wheel, " FR:", fr_wheel, " BL:", bl_wheel, " BR:", br_wheel)

func _find_wheel_by_name(wheel_name: String) -> LCWheelEffector:
	"""Helper to find wheel by name in parent"""
	if not parent:
		return null
	var wheel = parent.get_node_or_null(wheel_name)
	if wheel and wheel is LCWheelEffector:
		return wheel
	return null

# Processing physics for Rover controller
func _physics_process(_delta: float):
	# Only apply physics forces if we have authority
	# Remote clients will receive synchronized position/velocity from MultiplayerSynchronizer
	if not has_authority():
		return
		
	if parent and parent is VehicleBody3D:
		# Update speed and emit signal only on significant change
		current_speed = parent.linear_velocity.length()
		if abs(current_speed - prev_speed) > 0.01:
			prev_speed = current_speed
			speed_changed.emit(current_speed)

		# Apply control based on drive mode
		match drive_mode:
			0: # Standard
				_apply_standard_control()
			1: # Ackermann
				_apply_ackermann_control()
			2: # Differential
				_apply_differential_control()
			3: # Independent
				_apply_independent_control()
		
		# Apply slope compensation with reduced frequency
		slope_check_timer += _delta
		if slope_check_timer >= SLOPE_CHECK_INTERVAL:
			slope_check_timer = 0.0
			_check_slope_compensation()

func _apply_standard_control():
	"""Standard car steering: Only front wheels steer"""
	var speed_factor = _get_speed_factor()
	
	# Apply to parent (Front wheels via VehicleBody3D steering)
	# IMPORTANT: Invert motor direction to make W move toward the bumper
	parent.engine_force = -motor_input * ENGINE_FORCE * speed_factor
	
	# IMPORTANT: Invert steering to make A turn left and D turn right
	parent.steering = -steering_input * STEERING_FORCE
	
	# Apply brakes
	parent.brake = brake_input * BRAKE_FORCE
	
	# Ensure back wheels are straight and not driving individually
	if bl_wheel:
		bl_wheel.steering = 0.0
		bl_wheel.engine_force = 0.0
	if br_wheel:
		br_wheel.steering = 0.0
		br_wheel.engine_force = 0.0

	_emit_basic_signals()

func _apply_ackermann_control():
	"""
	Modified Ackermann control:
	- AD keys (steering_input): 4-Wheel Steering (Rotation)
	- QE keys (crab_input): Crab Steering (Translation)
	"""
	var speed_factor = _get_speed_factor()
	
	# Calculate steering angles
	# AD rotates: Front turns opposite to Back
	# QE crabs: Front and Back turn same direction
	var front_angle = (-steering_input + crab_input) * STEERING_FORCE
	var back_angle = (steering_input + crab_input) * STEERING_FORCE
	
	# Apply to parent (Front wheels via VehicleBody3D steering)
	parent.engine_force = -motor_input * ENGINE_FORCE * speed_factor
	parent.steering = front_angle
	parent.brake = brake_input * BRAKE_FORCE
	
	# Apply to back wheels manually
	if bl_wheel:
		bl_wheel.steering = back_angle
		bl_wheel.engine_force = 0.0 # Let parent handle drive force
	if br_wheel:
		br_wheel.steering = back_angle
		br_wheel.engine_force = 0.0

	_emit_basic_signals()

func _apply_differential_control():
	"""Tank-like steering: left/right wheels can rotate at different speeds"""
	var speed_factor = _get_speed_factor()
	
	# Don't use parent controls in differential mode
	parent.engine_force = 0.0
	parent.steering = 0.0
	parent.brake = 0.0
	
	# Calculate left and right motor forces
	var base_motor = -motor_input * ENGINE_FORCE * speed_factor
	var steering_differential = steering_input * ENGINE_FORCE * 0.5
	
	var left_motor = base_motor - steering_differential
	var right_motor = base_motor + steering_differential
	
	# Apply to individual wheels
	if fl_wheel: fl_wheel.engine_force = left_motor
	if bl_wheel: bl_wheel.engine_force = left_motor
	if fr_wheel: fr_wheel.engine_force = right_motor
	if br_wheel: br_wheel.engine_force = right_motor
	
	# Apply brakes to all wheels
	var brake_force = brake_input * BRAKE_FORCE
	if fl_wheel: fl_wheel.brake = brake_force
	if fr_wheel: fr_wheel.brake = brake_force
	if bl_wheel: bl_wheel.brake = brake_force
	if br_wheel: br_wheel.brake = brake_force

	_emit_basic_signals()

func _apply_independent_control():
	"""Full independent control: each wheel can be controlled separately"""
	if not enable_individual_control:
		_apply_differential_control()
		return
	
	# Don't use parent controls in independent mode
	parent.engine_force = 0.0
	parent.steering = 0.0
	parent.brake = 0.0
	
	# Apply individual wheel controls
	_apply_wheel_control(fl_wheel, wheel_controls["front_left"])
	_apply_wheel_control(fr_wheel, wheel_controls["front_right"])
	_apply_wheel_control(bl_wheel, wheel_controls["back_left"])
	_apply_wheel_control(br_wheel, wheel_controls["back_right"])

	_emit_basic_signals()

func _apply_wheel_control(wheel: LCWheelEffector, control: Dictionary):
	"""Apply control values to a specific wheel"""
	if not wheel:
		return
	
	var speed_factor = _get_speed_factor()
	wheel.engine_force = -control["motor"] * ENGINE_FORCE * speed_factor
	wheel.brake = control["brake"] * BRAKE_FORCE
	wheel.steering = control["steering"] * STEERING_FORCE

func _emit_basic_signals():
	# Emit signals only on significant changes
	if abs(motor_input - prev_motor_input) > 0.01:
		prev_motor_input = motor_input
		motor_state_changed.emit(motor_input)

	if abs(steering_input - prev_steering_input) > 0.01:
		prev_steering_input = steering_input
		steering_changed.emit(steering_input)
	
	if brake_input > 0:
		brake_applied.emit(brake_input)

func _get_speed_factor() -> float:
	"""Calculate speed-based scaling to prevent flipping"""
	if current_speed > 2.0:
		return 1.0 - min((current_speed - 2.0) / 3.0, 0.6)
	return 1.0

# Check and apply slope compensation to prevent flipping downhill
func _check_slope_compensation():
	if parent and parent.linear_velocity.length() > 1.0:
		var up = parent.global_transform.basis.y.normalized()
		var slope_dot = up.dot(Vector3.UP)
		
		# If we're on a significant slope
		if slope_dot < 0.9:
			# Automatically apply braking force proportional to the slope
			var auto_brake = (1.0 - slope_dot) * 0.7
			parent.brake = max(parent.brake, auto_brake * BRAKE_FORCE)
			
			# Reduce engine force on steep downhill slopes
			if motor_input < 0:  # Going downhill
				parent.engine_force *= slope_dot * 0.8

# Simple command methods
func set_motor(value: float):
	motor_input = clamp(value, -1.0, 1.0)

func set_steering(value: float):
	steering_input = clamp(value, -1.0, 1.0)

func set_crab_steering(value: float):
	crab_input = clamp(value, -1.0, 1.0)

func set_brake(value: float):
	brake_input = clamp(value, 0.0, 1.0)

# Individual Wheel Control Methods
func set_wheel_motor(wheel_name: String, value: float):
	if wheel_name in wheel_controls:
		wheel_controls[wheel_name]["motor"] = clamp(value, -1.0, 1.0)
		_emit_wheel_control_signal(wheel_name)

func set_wheel_brake(wheel_name: String, value: float):
	if wheel_name in wheel_controls:
		wheel_controls[wheel_name]["brake"] = clamp(value, 0.0, 1.0)
		_emit_wheel_control_signal(wheel_name)

func set_wheel_steering(wheel_name: String, value: float):
	if wheel_name in wheel_controls:
		wheel_controls[wheel_name]["steering"] = clamp(value, -1.0, 1.0)
		_emit_wheel_control_signal(wheel_name)

func _emit_wheel_control_signal(wheel_name: String):
	var control = wheel_controls[wheel_name]
	wheel_control_changed.emit(wheel_name, control["motor"], control["brake"], control["steering"])

# Control life cycle
func take_control():
	_reset_inputs()

func release_control():
	_reset_inputs()

# Private helper to reset all inputs and parent vehicle state
func _reset_inputs():
	motor_input = 0.0
	steering_input = 0.0
	crab_input = 0.0
	brake_input = 0.0
	
	# Reset individual wheel controls
	for wheel_name in wheel_controls:
		wheel_controls[wheel_name] = {"motor": 0.0, "brake": 0.0, "steering": 0.0}
	
	# Make sure parent values are reset too
	if parent and parent is VehicleBody3D:
		parent.engine_force = 0.0
		parent.steering = 0.0
		parent.brake = 0.0
		
		# Reset individual wheels if they exist
		if fl_wheel:
			fl_wheel.engine_force = 0.0
			fl_wheel.brake = 0.0
		if fr_wheel:
			fr_wheel.engine_force = 0.0
			fr_wheel.brake = 0.0
		if bl_wheel:
			bl_wheel.engine_force = 0.0
			bl_wheel.brake = 0.0
		if br_wheel:
			br_wheel.engine_force = 0.0
			br_wheel.brake = 0.0

# Command Methods (Reflection)
func cmd_set_motor(value: float = 0.0):
	set_motor(value)

func cmd_set_steering(value: float = 0.0):
	set_steering(value)

func cmd_set_crab_steering(value: float = 0.0):
	set_crab_steering(value)

func cmd_set_brake(value: float = 0.0):
	set_brake(value)

func cmd_take_image():
	# Find camera effector in children or descendants
	var camera = _find_camera(parent if parent else self)
	if camera:
		return await camera.cmd_take_image()
	return "No camera found on rover"

func _find_camera(node: Node) -> Node:
	if node is LCCameraEffector:
		return node
	for child in node.get_children():
		var found = _find_camera(child)
		if found:
			return found
	return null
