class_name LCRoverInputMappings
extends Node

# This script sets up input mappings for the rover at runtime
# It ensures all necessary input actions are available

func _ready():
	setup_input_mappings()
	
func setup_input_mappings():
	# Make sure we have all the necessary input actions for rover control
	
	print("LCRoverInputMappings: Setting up input mappings")
	
	# Movement controls
	if not InputMap.has_action("move_forward"):
		InputMap.add_action("move_forward")
		var event = InputEventKey.new()
		event.keycode = KEY_W
		InputMap.action_add_event("move_forward", event)
	
	if not InputMap.has_action("move_backward"):
		InputMap.add_action("move_backward")
		var event = InputEventKey.new()
		event.keycode = KEY_S
		InputMap.action_add_event("move_backward", event)
	
	if not InputMap.has_action("move_left"):
		InputMap.add_action("move_left")
		var event = InputEventKey.new()
		event.keycode = KEY_A
		InputMap.action_add_event("move_left", event)
	
	if not InputMap.has_action("move_right"):
		InputMap.add_action("move_right")
		var event = InputEventKey.new()
		event.keycode = KEY_D
		InputMap.action_add_event("move_right", event)
	
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
	
	# Brake
	if not InputMap.has_action("brake"):
		InputMap.add_action("brake")
		var event = InputEventKey.new()
		event.keycode = KEY_SPACE
		InputMap.action_add_event("brake", event)
	
	# Gamepad controls
	if not InputMap.has_action("gamepad_left"):
		InputMap.add_action("gamepad_left")
		var event = InputEventJoypadMotion.new()
		event.axis = JOY_AXIS_LEFT_X
		event.axis_value = -1.0
		InputMap.action_add_event("gamepad_left", event)
	
	if not InputMap.has_action("gamepad_right"):
		InputMap.add_action("gamepad_right")
		var event = InputEventJoypadMotion.new()
		event.axis = JOY_AXIS_LEFT_X
		event.axis_value = 1.0
		InputMap.action_add_event("gamepad_right", event)
	
	if not InputMap.has_action("gamepad_forward"):
		InputMap.add_action("gamepad_forward")
		var event = InputEventJoypadMotion.new()
		event.axis = JOY_AXIS_LEFT_Y
		event.axis_value = -1.0
		InputMap.action_add_event("gamepad_forward", event)
	
	if not InputMap.has_action("gamepad_backward"):
		InputMap.add_action("gamepad_backward")
		var event = InputEventJoypadMotion.new()
		event.axis = JOY_AXIS_LEFT_Y
		event.axis_value = 1.0
		InputMap.action_add_event("gamepad_backward", event)
	
	print("LCRoverInputMappings: Input mappings set up successfully") 
