class_name LCSpacecraftInputAdapter
extends LCInputAdapter

# target is inherited from LCInputAdapter

func _ready():
	pass # print("SpacecraftInputAdapter ready. Target: ", target)

# Previous state for change detection
var _prev_throttle := false
var _prev_torque := Vector3.ZERO

func _input(_event):
	var _target = get_resolved_target()
	
	# Only process input if the target is a spacecraft controller
	var is_compatible_controller = _target is LCSpacecraftController
	
	if is_compatible_controller:
		# Check if input is captured by UI
		if not should_process_input(_event):
			if _prev_throttle:
				_prev_throttle = false
				_send_command("THROTTLE", {"enabled": false})
			if _prev_torque != Vector3.ZERO:
				_prev_torque = Vector3.ZERO
				_send_command("ORIENTATION", {"x": 0.0, "y": 0.0, "z": 0.0})
			return

		# Throttle input
		var new_throttle = _prev_throttle
		if _event.is_action_pressed("throttle", true):
			new_throttle = true
		elif _event.is_action_released("throttle"):
			new_throttle = false
		
		if new_throttle != _prev_throttle:
			_prev_throttle = new_throttle
			_send_command("THROTTLE", {"enabled": new_throttle})

		# Torque input
		var torque_action := Vector3(
			- Input.get_action_strength("pitch_up") + Input.get_action_strength("pitch_down"),
			- Input.get_action_strength("yaw_right") + Input.get_action_strength("yaw_left"),
			- Input.get_action_strength("roll_cw") + Input.get_action_strength("roll_ccw")
		)

		if not torque_action.is_equal_approx(_prev_torque):
			_prev_torque = torque_action
			_send_command("ORIENTATION", {"x": torque_action.x, "y": torque_action.y, "z": torque_action.z})

func _send_command(cmd_name: String, args: Dictionary):
	var _target = get_resolved_target()
	if not _target: return
	
	var cmd = LCCommand.new(cmd_name, _target.get_path(), args, "local")
	LCCommandRouter.submit(cmd)
