extends LCInputAdapter

## Input adapter for Avatar movement when no entity is controlled
## Reads movement input and sends it to LCAvatarController

func _process(_delta):
	var _target = get_resolved_target()
	
	# Only process if target is AvatarController
	if not _target is LCAvatarController:
		return
	
	# Read movement input
	var motion_direction := Vector3(
		Input.get_action_strength("move_left") - Input.get_action_strength("move_right"),
		Input.get_action_strength("move_up") - Input.get_action_strength("move_down"),
		Input.get_action_strength("move_forward") - Input.get_action_strength("move_back")
	)
	
	# Send to controller
	_target.set_direction(motion_direction)
	
	# Apply speed modifiers based on key presses
	if Input.is_key_pressed(KEY_ALT):
		_target.set_speed(100)  # Fast mode
	elif Input.is_key_pressed(KEY_SHIFT):
		_target.set_speed(20)   # Medium mode
	else:
		_target.set_speed(10)   # Normal mode
