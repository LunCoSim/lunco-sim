class_name LCOperatorInputAdapter
extends LCInputAdapter

## Input adapter for Operator controller
## Handles movement input and camera-based orientation

@export var camera: SpringArmCamera

# target is inherited from LCInputAdapter

# Previous state for change detection
var _prev_move_vec := Vector3.ZERO

func _input(_event):
	var _target = get_resolved_target()
	
	# Check if we have a compatible operator controller
	var is_compatible_controller = _target is LCOperatorController
	
	# Only process input if the target is an operator controller
	if is_compatible_controller:
		# Check if input is captured by UI
		if not should_process_input(_event):
			if _prev_move_vec != Vector3.ZERO:
				_prev_move_vec = Vector3.ZERO
				_send_command("MOVE", {"x": 0, "y": 0, "z": 0})
			return

		# Reset position on R key
		if Input.is_action_just_pressed("reset_position"):
			_send_command("RESET_POSITION", {})

		var input_vec := Vector3(
			Input.get_action_strength("move_left") - Input.get_action_strength("move_right"),
			Input.get_action_strength("move_up") - Input.get_action_strength("move_down"),
			Input.get_action_strength("move_forward") - Input.get_action_strength("move_back")
		)
		
		# Calculate world space move vector using camera orientation
		var move_vec_world = input_vec
		if camera:
			var basis = camera.get_plain_basis()
			move_vec_world = basis * input_vec
			
		if input_vec.length() > 1.0:
			move_vec_world = move_vec_world.normalized()
			
		if not move_vec_world.is_equal_approx(_prev_move_vec):
			_prev_move_vec = move_vec_world
			_send_command("MOVE", {"x": move_vec_world.x, "y": move_vec_world.y, "z": move_vec_world.z})

func _send_command(cmd_name: String, args: Dictionary):
	var _target = get_resolved_target()
	if not _target: return
	
	var cmd = LCCommand.new(cmd_name, _target.get_path(), args, "local")
	LCCommandRouter.submit(cmd)
