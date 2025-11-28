class_name LCRoverInputAdapter
extends LCInputAdapter

# target is inherited from LCInputAdapter

# Input sensitivity and deadzone settings
@export var MOTOR_SENSITIVITY := 1.0
@export var STEERING_SENSITIVITY := 1.0
@export var INPUT_DEADZONE := 0.1

func _ready():
	_setup_input_mappings()

func _setup_input_mappings():
	# Crab Steering
	if not InputMap.has_action("crab_left"):
		InputMap.add_action("crab_left")
		var event = InputEventKey.new()
		event.keycode = KEY_E
		InputMap.action_add_event("crab_left", event)
	
	if not InputMap.has_action("crab_right"):
		InputMap.add_action("crab_right")
		var event = InputEventKey.new()
		event.keycode = KEY_Q
		InputMap.action_add_event("crab_right", event)



func _input(_event):
	# Only process inputs if we have a target
	if not target:
		return
		
	var _target = get_resolved_target()
		
	# Check if we have a compatible rover controller
	var is_compatible_controller = _target is LCRoverController or _target is LCRoverJointController
	
	# Only process input if the target is a rover controller
	if is_compatible_controller:
		# Check if input is captured by UI
		if not should_process_input():
			if _target.has_method("set_motor"): _target.set_motor(0)
			if _target.has_method("set_steering"): _target.set_steering(0)
			if _target.has_method("set_crab_steering"): _target.set_crab_steering(0)
			if _target.has_method("set_brake"): _target.set_brake(0)
			return

		# Process movement input
		var motor_input = Input.get_action_strength("move_forward") - Input.get_action_strength("move_backward")
		var steering_input = Input.get_action_strength("move_right") - Input.get_action_strength("move_left")
		var crab_input = Input.get_action_strength("crab_right") - Input.get_action_strength("crab_left")
		var brake_input = Input.get_action_strength("brake")
		
		# Process gamepad input if available
		var gamepad_movement = Input.get_vector("gamepad_left", "gamepad_right", "gamepad_forward", "gamepad_backward")
		if gamepad_movement.length() > INPUT_DEADZONE:
			motor_input = gamepad_movement.y if abs(gamepad_movement.y) > abs(motor_input) else motor_input
			steering_input = gamepad_movement.x if abs(gamepad_movement.x) > abs(steering_input) else steering_input
		
		# Apply inputs to controller
		if _target.has_method("set_motor"):
			_target.set_motor(motor_input * MOTOR_SENSITIVITY)
		
		if _target.has_method("set_steering"):
			_target.set_steering(steering_input * STEERING_SENSITIVITY)
			
		if _target.has_method("set_crab_steering"):
			_target.set_crab_steering(crab_input * STEERING_SENSITIVITY)
		
		if _target.has_method("set_brake"):
			_target.set_brake(brake_input) 
