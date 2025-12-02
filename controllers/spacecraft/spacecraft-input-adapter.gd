class_name LCSpacecraftInputAdapter
extends LCInputAdapter

# target is inherited from LCInputAdapter

func _ready():
	print("SpacecraftInputAdapter ready. Target: ", target)

func _input(_event):
	var _target = get_resolved_target()
	
	# Debug: Print when we have a spacecraft controller
	if _target and (_target.has_method("throttle") or _target.get_class() == "LCSpacecraftController" or _target is LCSpacecraftController):
		# Only print on key presses to avoid spam
		if _event is InputEventKey and _event.pressed and not _event.is_echo():
			print("SpacecraftInputAdapter: Processing input for ", _target.get_parent().name)
		
		# Check if input is captured by UI
		if not should_process_input():
			_target.throttle(false)
			_target.change_orientation(Vector3.ZERO)
			return

		if Input.is_action_just_pressed("throttle"):
			print("SpacecraftInputAdapter: Throttle ON")
			_target.throttle(true)
		elif Input.is_action_just_released("throttle"):
			print("SpacecraftInputAdapter: Throttle OFF")
			_target.throttle(false)

		var torque_action := Vector3(
			- Input.get_action_strength("pitch_up") + Input.get_action_strength("pitch_down"),
			- Input.get_action_strength("yaw_right") + Input.get_action_strength("yaw_left"),
			- Input.get_action_strength("roll_cw") + Input.get_action_strength("roll_ccw")
		)

		if torque_action != Vector3.ZERO:
			print("SpacecraftInputAdapter: Torque ", torque_action)
		_target.change_orientation(torque_action)
	elif _target != null and (_event is InputEventKey and _event.pressed and not _event.is_echo()):
		# Debug: Print what type of target we have instead
		print("SpacecraftInputAdapter: Target is ", _target.get_class(), " not LCSpacecraftController")
