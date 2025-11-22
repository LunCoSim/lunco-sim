class_name LCOperatorInputAdapter
extends LCInputAdapter

# target is inherited from LCInputAdapter

func _input(_event):
	var _target = get_resolved_target()
	
	if _target is LCOperatorController:
		if Input.is_action_just_pressed("reset_position"):
			_target.reset_position();

		var motion_direction := Vector3(
			Input.get_action_strength("move_left") - Input.get_action_strength("move_right"),
			Input.get_action_strength("move_up") - Input.get_action_strength("move_down"),
			Input.get_action_strength("move_forward") - Input.get_action_strength("move_back")
		)

		_target.move(motion_direction.normalized())	
