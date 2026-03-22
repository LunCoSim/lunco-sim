class_name LCRoverRigidInputAdapter
extends LCInputAdapter

# Previous state for change detection
var _prev_motor_input := 0.0
var _prev_steering_input := 0.0
var _prev_brake_input := 0.0

func _ready() -> void:
	if not target:
		target = get_parent()

func _physics_process(_delta):
	var _target = get_resolved_target()
	if not _target or not _target is LCRoverRigidController:
		return

	# Poll movement input
	var motor_input = Input.get_action_strength("move_forward") - Input.get_action_strength("move_backward")
	var steering_input = Input.get_action_strength("move_right") - Input.get_action_strength("move_left")
	var brake_input = Input.get_action_strength("brake")
	
	# Change detection to prevent flooding the CommandRouter
	if not is_equal_approx(motor_input, _prev_motor_input):
		_prev_motor_input = motor_input
		_send_command("SET_MOTOR", {"value": motor_input})
			
	if not is_equal_approx(steering_input, _prev_steering_input):
		_prev_steering_input = steering_input
		_send_command("SET_STEERING", {"value": steering_input})
			
	if not is_equal_approx(brake_input, _prev_brake_input):
		_prev_brake_input = brake_input
		_send_command("SET_BRAKE", {"value": brake_input})

func _send_command(cmd_name: String, args: Dictionary):
	var _target = get_resolved_target()
	if not _target: return
	
	var cmd = LCCommand.new(cmd_name, _target.get_path(), args, "local")
	LCCommandRouter.submit(cmd)
