class_name LCOperatorInputAdapter
extends LCInputAdapter

## Input adapter for Operator controller
## Handles movement input and camera-based orientation

@export var camera: SpringArmCamera

# target is inherited from LCInputAdapter

func _input(_event):
	# Only process inputs if we have a target
	if not target:
		return
		
	var _target = get_resolved_target()
	
	# Check if we have a compatible operator controller
	var is_compatible_controller = _target is LCOperatorController
	
	# Only process input if the target is an operator controller
	if is_compatible_controller:
		# Check if input is captured by UI
		if not should_process_input():
			_target.move(Vector3.ZERO)
			return

		# Update orientation based on camera
		if camera:
			_target.orient(camera.get_plain_basis())
		
		# Reset position on R key
		if Input.is_action_just_pressed("reset_position"):
			_target.reset_position()

		var motion_direction := Vector3(
			Input.get_action_strength("move_left") - Input.get_action_strength("move_right"),
			Input.get_action_strength("move_up") - Input.get_action_strength("move_down"),
			Input.get_action_strength("move_forward") - Input.get_action_strength("move_back")
		)

		_target.move(motion_direction.normalized())
