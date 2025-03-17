extends Node

@export var controller: LC3DSimRoverController

# Input sensitivity and deadzone settings
@export var MOTOR_SENSITIVITY := 1.0
@export var STEERING_SENSITIVITY := 1.0
@export var INPUT_DEADZONE := 0.1

# Input state
var forward_input := 0.0
var reverse_input := 0.0
var left_input := 0.0
var right_input := 0.0
var brake_input := 0.0

# Track brake state for debug output
var was_braking := false

func _ready():
	if not controller:
		push_warning("RoverInputAdapter: No controller reference set!")
		# Try to autodetect controller if not set
		controller = get_node_or_null("../RoverController")
		if controller:
			print("RoverInputAdapter: Auto-detected controller")

func _physics_process(delta: float):
	if not controller:
		return
		
	process_keyboard_input()
	process_gamepad_input()
	
	# Apply processed inputs to controller
	apply_inputs()

func process_keyboard_input():
	# Forward/Reverse
	forward_input = Input.get_action_strength("move_forward")
	reverse_input = Input.get_action_strength("move_backward")
	
	# Left/Right
	left_input = Input.get_action_strength("move_left")
	right_input = Input.get_action_strength("move_right")
	
	# Check multiple possible brake actions
	var space_brake = Input.get_action_strength("brake")
	var jump_brake = Input.get_action_strength("jump")
	var throttle_brake = Input.get_action_strength("throttle")
	
	# Use the strongest brake input
	brake_input = max(space_brake, max(jump_brake, throttle_brake))
	
	# Debug info for braking
	if brake_input > 0 and not was_braking:
		print("Brake applied: ", brake_input)
		was_braking = true
	elif brake_input == 0 and was_braking:
		was_braking = false

func process_gamepad_input():
	# Get gamepad analog stick values if available
	var gamepad_movement = Input.get_vector("gamepad_left", "gamepad_right", "gamepad_forward", "gamepad_backward")
	
	if gamepad_movement.length() > INPUT_DEADZONE:
		# Override keyboard input with gamepad if stick is moved
		forward_input = max(forward_input, gamepad_movement.y if gamepad_movement.y > 0 else 0)
		reverse_input = max(reverse_input, -gamepad_movement.y if gamepad_movement.y < 0 else 0)
		left_input = max(left_input, -gamepad_movement.x if gamepad_movement.x < 0 else 0)
		right_input = max(right_input, gamepad_movement.x if gamepad_movement.x > 0 else 0)

func apply_inputs():
	# Only proceed if we have a controller
	if not controller:
		return
		
	# Calculate motor input (-1 to 1)
	var motor = (forward_input - reverse_input) * MOTOR_SENSITIVITY
	
	# Strong braking: reduce motor force when brake is applied
	if brake_input > 0:
		motor *= (1.0 - brake_input)
	
	controller.set_motor(motor)
	
	# Calculate steering input (-1 to 1)
	var steering = (right_input - left_input) * STEERING_SENSITIVITY
	controller.set_steering(steering)
	
	# Apply brake - full brake value for stronger braking response
	controller.set_brake(brake_input) 
