@icon("res://controllers/rover/rover.svg")
class_name LCRoverController
extends LCController

# Export categories for easy configuration in the editor
@export_category("Rover Movement Parameters")
@export var ENGINE_FORCE := 1200.0  # Reduced force to prevent flipping
@export var STEERING_FORCE := 0.6  # Increased for better steering response
@export var MAX_SPEED := 3.5  # Realistic max speed for lunar rover
@export var BRAKE_FORCE := 800.0  # Increased braking for better control

# Get the parent VehicleBody3D node
@onready var parent: VehicleBody3D:
	get:
		var p = self.get_parent()
		if p and p is VehicleBody3D:
			return p
		else:
			push_error("RoverController: Parent is not a VehicleBody3D! Got: " + str(p))
			return null

# Internal state
var motor_input := 0.0
var steering_input := 0.0
var brake_input := 0.0
var current_speed := 0.0

# Previous values for change detection
var prev_motor_input := 0.0
var prev_steering_input := 0.0
var prev_speed := 0.0

# Slope compensation optimization
var slope_check_timer := 0.0
const SLOPE_CHECK_INTERVAL := 0.2  # Check slope every 200ms instead of every physics frame

var debug_counter := 0

# Signals
signal motor_state_changed(power: float)
signal steering_changed(angle: float)
signal speed_changed(speed: float)
signal brake_applied(force: float)

# Initialize the controller
func _ready():
	# Ensure we're in the right group for discovery
	if not is_in_group("RoverControllers"):
		add_to_group("RoverControllers")
	
	# Reset inputs on start
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0
	
	# Ensure parent is a VehicleBody3D
	if not parent is VehicleBody3D:
		push_error("RoverController's parent must be a VehicleBody3D")
	else:
		# Directly set initial values
		parent.engine_force = 0.0
		parent.steering = 0.0
		parent.brake = 0.0
	
	# Add command executor
	var executor = LCCommandExecutor.new()
	executor.name = "CommandExecutor"
	add_child(executor)
	print("DEBUG: RoverController _ready complete. Created executor: ", executor, " path: ", executor.get_path())



# Processing physics for Rover controller
func _physics_process(_delta: float):
	# Only apply physics forces if we have authority
	# Remote clients will receive synchronized position/velocity from MultiplayerSynchronizer
	if not has_authority():
		return
		
	if parent and parent is VehicleBody3D:
		# Calculate speed-based engine scaling to prevent flip at high speeds
		var speed_factor = 1.0
		if current_speed > 2.0:
			speed_factor = 1.0 - min((current_speed - 2.0) / 3.0, 0.6)
			
		# IMPORTANT: Invert motor direction to make W move toward the red bumper
		# Apply speed-based scaling to prevent flipping
		parent.engine_force = -motor_input * ENGINE_FORCE * speed_factor
		
		# IMPORTANT: Invert steering to make A turn left and D turn right
		parent.steering = -steering_input * STEERING_FORCE
		
		# Apply brakes if needed
		parent.brake = brake_input * BRAKE_FORCE
		if brake_input > 0:
			brake_applied.emit(brake_input)

		# Update speed and emit signal only on significant change
		current_speed = parent.linear_velocity.length()
		if abs(current_speed - prev_speed) > 0.01:
			prev_speed = current_speed
			speed_changed.emit(current_speed)

		# Emit signals only on significant changes
		if abs(motor_input - prev_motor_input) > 0.01:
			prev_motor_input = motor_input
			motor_state_changed.emit(motor_input)

		if abs(steering_input - prev_steering_input) > 0.01:
			prev_steering_input = steering_input
			steering_changed.emit(steering_input)
		
		# Apply slope compensation with reduced frequency
		slope_check_timer += _delta
		if slope_check_timer >= SLOPE_CHECK_INTERVAL:
			slope_check_timer = 0.0
			_check_slope_compensation()


# Check and apply slope compensation to prevent flipping downhill
func _check_slope_compensation():
	if parent and parent.linear_velocity.length() > 1.0:
		var up = parent.global_transform.basis.y.normalized()
		var slope_dot = up.dot(Vector3.UP)
		
		# If we're on a significant slope
		if slope_dot < 0.9:
			# Automatically apply braking force proportional to the slope
			var auto_brake = (1.0 - slope_dot) * 0.7  # Increased from 0.5 to 0.7
			parent.brake = max(parent.brake, auto_brake * BRAKE_FORCE)
			
			# Reduce engine force on steep downhill slopes
			if motor_input < 0:  # Going downhill
				parent.engine_force *= slope_dot * 0.8  # Added extra reduction factor

# Simple command methods
func set_motor(value: float):
	motor_input = clamp(value, -1.0, 1.0)
	
	# Speed-based engine force scaling to prevent flipping at higher speeds
	var speed_factor = 1.0
	if current_speed > 2.0:
		speed_factor = 1.0 - min((current_speed - 2.0) / 3.0, 0.6)
	
	# Immediately apply engine force if we have a parent
	if parent and parent is VehicleBody3D:
		# IMPORTANT: Invert motor direction
		parent.engine_force = -motor_input * ENGINE_FORCE * speed_factor

func set_steering(value: float):
	steering_input = clamp(value, -1.0, 1.0)
	# Immediately apply steering if we have a parent
	if parent and parent is VehicleBody3D:
		# IMPORTANT: Invert steering direction
		parent.steering = -steering_input * STEERING_FORCE

func set_brake(value: float):
	brake_input = clamp(value, 0.0, 1.0)
	# Immediately apply brake if we have a parent
	if parent and parent is VehicleBody3D:
		parent.brake = brake_input * BRAKE_FORCE

# Simplified control methods (required for compatibility with signals)
func take_control():
	_reset_inputs()

func release_control():
	_reset_inputs()

# Private helper to reset all inputs and parent vehicle state
func _reset_inputs():
	# Reset all inputs when taking/releasing control
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0
	
	# Make sure parent values are reset too
	if parent and parent is VehicleBody3D:
		parent.engine_force = 0.0
		parent.steering = 0.0
		parent.brake = 0.0
# Command Methods (Reflection)
func cmd_set_motor(args: Dictionary):
	set_motor(args.get("value", 0.0))

func cmd_set_steering(args: Dictionary):
	set_steering(args.get("value", 0.0))

func cmd_set_crab_steering(args: Dictionary):
	if has_method("set_crab_steering"):
		call("set_crab_steering", args.get("value", 0.0))

func cmd_set_brake(args: Dictionary):
	set_brake(args.get("value", 0.0))
