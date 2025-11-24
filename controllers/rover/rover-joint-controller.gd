@icon("res://controllers/rover/rover.svg")
class_name LCRoverJointController
extends LCController

## Advanced rover controller with individual wheel/joint control
## Supports multiple drive modes: Ackermann, Differential, and Independent

# Export categories for easy configuration in the editor
@export_category("Drive Configuration")
@export_enum("Standard:0", "Ackermann:1", "Differential:2", "Independent:3") var drive_mode: int = 0
@export var enable_individual_control: bool = false

@export_category("Rover Movement Parameters")
@export var ENGINE_FORCE := 1200.0
@export var STEERING_FORCE := 0.6
@export var MAX_SPEED := 3.5
@export var BRAKE_FORCE := 800.0

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
			push_error("RoverJointController: Parent is not a VehicleBody3D! Got: " + str(p))
			return null

# Wheel references
var fl_wheel: LCWheelEffector
var fr_wheel: LCWheelEffector
var bl_wheel: LCWheelEffector
var br_wheel: LCWheelEffector

# Control inputs
var motor_input := 0.0
var steering_input := 0.0
var crab_input := 0.0
var brake_input := 0.0
var current_speed := 0.0

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

func _ready():
	if not is_in_group("RoverControllers"):
		add_to_group("RoverControllers")
	
	# Find wheel references
	_discover_wheels()
	
	# Reset inputs
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0
	
	# Ensure parent is a VehicleBody3D
	if not parent is VehicleBody3D:
		push_error("RoverJointController's parent must be a VehicleBody3D")

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
		push_warning("RoverJointController: Not all wheels found! Individual control may not work.")
		print("  FL: ", fl_wheel, " FR: ", fr_wheel, " BL: ", bl_wheel, " BR: ", br_wheel)

func _find_wheel_by_name(wheel_name: String) -> LCWheelEffector:
	"""Helper to find wheel by name in parent"""
	if not parent:
		return null
	var wheel = parent.get_node_or_null(wheel_name)
	if wheel and wheel is LCWheelEffector:
		return wheel
	return null

func _physics_process(_delta: float):
	if not has_authority():
		return
	
	if parent and parent is VehicleBody3D:
		current_speed = parent.linear_velocity.length()
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
		
		# Apply slope compensation
		_apply_slope_compensation()

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
		
	motor_state_changed.emit(motor_input)
	steering_changed.emit(steering_input)
	if brake_input > 0:
		brake_applied.emit(brake_input)

func _apply_differential_control():
	"""Tank-like steering: left/right wheels can rotate at different speeds"""
	var speed_factor = _get_speed_factor()
	
	# Don't use parent controls in differential mode
	parent.engine_force = 0.0
	parent.steering = 0.0
	parent.brake = 0.0
	
	# Calculate left and right motor forces
	# steering_input affects the differential between left and right
	var base_motor = -motor_input * ENGINE_FORCE * speed_factor
	var steering_differential = steering_input * ENGINE_FORCE * 0.5
	
	var left_motor = base_motor - steering_differential
	var right_motor = base_motor + steering_differential
	
	# Apply to individual wheels
	if fl_wheel:
		fl_wheel.engine_force = left_motor
	else:
		push_warning("RoverJointController: FL wheel not found!")
	if bl_wheel:
		bl_wheel.engine_force = left_motor
	else:
		push_warning("RoverJointController: BL wheel not found!")
	if fr_wheel:
		fr_wheel.engine_force = right_motor
	else:
		push_warning("RoverJointController: FR wheel not found!")
	if br_wheel:
		br_wheel.engine_force = right_motor
	else:
		push_warning("RoverJointController: BR wheel not found!")
	
	# Debug: Print once to verify
	if Engine.get_physics_frames() % 60 == 0 and motor_input != 0:
		print("Differential mode - Left: %.1f, Right: %.1f" % [left_motor, right_motor])
	
	# Apply brakes to all wheels
	var brake_force = brake_input * BRAKE_FORCE
	if fl_wheel:
		fl_wheel.brake = brake_force
	if fr_wheel:
		fr_wheel.brake = brake_force
	if bl_wheel:
		bl_wheel.brake = brake_force
	if br_wheel:
		br_wheel.brake = brake_force
	
	motor_state_changed.emit(motor_input)
	steering_changed.emit(steering_input)
	if brake_input > 0:
		brake_applied.emit(brake_input)

func _apply_independent_control():
	"""Full independent control: each wheel can be controlled separately"""
	if not enable_individual_control:
		# Fall back to differential if individual control not enabled
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
	
	motor_state_changed.emit(motor_input)
	steering_changed.emit(steering_input)
	if brake_input > 0:
		brake_applied.emit(brake_input)

func _apply_standard_control():
	"""Standard car steering: Only front wheels steer"""
	var speed_factor = _get_speed_factor()
	
	# Apply to parent (Front wheels via VehicleBody3D steering)
	parent.engine_force = -motor_input * ENGINE_FORCE * speed_factor
	parent.steering = -steering_input * STEERING_FORCE
	parent.brake = brake_input * BRAKE_FORCE
	
	# Ensure back wheels are straight
	if bl_wheel:
		bl_wheel.steering = 0.0
		bl_wheel.engine_force = 0.0
	if br_wheel:
		br_wheel.steering = 0.0
		br_wheel.engine_force = 0.0
		
	motor_state_changed.emit(motor_input)
	steering_changed.emit(steering_input)
	if brake_input > 0:
		brake_applied.emit(brake_input)

func _apply_wheel_control(wheel: LCWheelEffector, control: Dictionary):
	"""Apply control values to a specific wheel"""
	if not wheel:
		return
	
	var speed_factor = _get_speed_factor()
	wheel.engine_force = -control["motor"] * ENGINE_FORCE * speed_factor
	wheel.brake = control["brake"] * BRAKE_FORCE
	wheel.steering = control["steering"] * STEERING_FORCE

func _get_speed_factor() -> float:
	"""Calculate speed-based scaling to prevent flipping"""
	if current_speed > 2.0:
		return 1.0 - min((current_speed - 2.0) / 3.0, 0.6)
	return 1.0

func _apply_slope_compensation():
	"""Prevent flipping on slopes"""
	if parent and parent.linear_velocity.length() > 1.0:
		var up = parent.global_transform.basis.y.normalized()
		var slope_dot = up.dot(Vector3.UP)
		
		if slope_dot < 0.9:
			var auto_brake = (1.0 - slope_dot) * 0.7
			parent.brake = max(parent.brake, auto_brake * BRAKE_FORCE)
			
			if motor_input < 0:
				parent.engine_force *= slope_dot * 0.8

# ============================================================================
# Public API - Simple Control Methods
# ============================================================================

func set_motor(value: float):
	"""Set motor input for all wheels (Ackermann/Differential modes)"""
	motor_input = clamp(value, -1.0, 1.0)

func set_steering(value: float):
	"""Set steering input"""
	steering_input = clamp(value, -1.0, 1.0)

func set_crab_steering(value: float):
	"""Set crab steering input"""
	crab_input = clamp(value, -1.0, 1.0)

func set_brake(value: float):
	"""Set brake input for all wheels"""
	brake_input = clamp(value, 0.0, 1.0)

# ============================================================================
# Public API - Individual Wheel Control
# ============================================================================

func set_wheel_motor(wheel_name: String, value: float):
	"""Set motor for individual wheel (Independent mode only)"""
	if wheel_name in wheel_controls:
		wheel_controls[wheel_name]["motor"] = clamp(value, -1.0, 1.0)
		_emit_wheel_control_signal(wheel_name)

func set_wheel_brake(wheel_name: String, value: float):
	"""Set brake for individual wheel (Independent mode only)"""
	if wheel_name in wheel_controls:
		wheel_controls[wheel_name]["brake"] = clamp(value, 0.0, 1.0)
		_emit_wheel_control_signal(wheel_name)

func set_wheel_steering(wheel_name: String, value: float):
	"""Set steering for individual wheel (Independent mode only)"""
	if wheel_name in wheel_controls:
		wheel_controls[wheel_name]["steering"] = clamp(value, -1.0, 1.0)
		_emit_wheel_control_signal(wheel_name)

func get_wheel_control(wheel_name: String) -> Dictionary:
	"""Get current control values for a wheel"""
	if wheel_name in wheel_controls:
		return wheel_controls[wheel_name].duplicate()
	return {}

func get_wheel_telemetry(wheel_name: String) -> Dictionary:
	"""Get telemetry data from a specific wheel"""
	var wheel = _get_wheel_by_name(wheel_name)
	if wheel and "Telemetry" in wheel:
		return wheel.Telemetry.duplicate()
	return {}

func _get_wheel_by_name(wheel_name: String) -> LCWheelEffector:
	"""Helper to get wheel reference by name"""
	match wheel_name:
		"front_left":
			return fl_wheel
		"front_right":
			return fr_wheel
		"back_left":
			return bl_wheel
		"back_right":
			return br_wheel
	return null

func _emit_wheel_control_signal(wheel_name: String):
	"""Emit signal when wheel control changes"""
	var control = wheel_controls[wheel_name]
	wheel_control_changed.emit(wheel_name, control["motor"], control["brake"], control["steering"])

# ============================================================================
# Control Lifecycle
# ============================================================================

func take_control():
	_reset_inputs()

func release_control():
	_reset_inputs()

func _reset_inputs():
	motor_input = 0.0
	steering_input = 0.0
	crab_input = 0.0
	brake_input = 0.0
	
	# Reset individual wheel controls
	for wheel_name in wheel_controls:
		wheel_controls[wheel_name] = {"motor": 0.0, "brake": 0.0, "steering": 0.0}
	
	# Reset parent vehicle state
	if parent and parent is VehicleBody3D:
		parent.engine_force = 0.0
		parent.steering = 0.0
		parent.brake = 0.0
		
		# Reset individual wheels
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
