extends Node

@onready var target:LCSpacecraftController=get_parent()
	
func _input(_event):
	if target:
		if Input.is_action_just_pressed("throttle"):
			target.throttle(true)
		elif Input.is_action_just_released("throttle"):
			target.throttle(false)

		var torque_action := Vector3(
			Input.get_action_strength("pitch_up") - Input.get_action_strength("pitch_down"),
			Input.get_action_strength("yaw_right") - Input.get_action_strength("yaw_left"),
			Input.get_action_strength("roll_cw") - Input.get_action_strength("roll_ccw")
		)

		target.change_orientation(torque_action)
