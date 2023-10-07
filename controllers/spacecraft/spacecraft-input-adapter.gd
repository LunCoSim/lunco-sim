class_name LCSpacecraftInputAdapter
extends Node

@export var target: Node

func _input(_event):
	var _target = target
	if target is LCAvatar:
		_target = target.target
		
	if _target is LCSpacecraftController:
		if Input.is_action_just_pressed("throttle"):
			_target.throttle(true)
		elif Input.is_action_just_released("throttle"):
			_target.throttle(false)

		var torque_action := Vector3(
			- Input.get_action_strength("pitch_up") + Input.get_action_strength("pitch_down"),
			- Input.get_action_strength("yaw_right") + Input.get_action_strength("yaw_left"),
			- Input.get_action_strength("roll_cw") + Input.get_action_strength("roll_ccw")
		)

		_target.change_orientation(torque_action)
