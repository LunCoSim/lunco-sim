extends Node

export (NodePath) var Cam #camera
export (NodePath) var Ward
export (NodePath) var Inputs

onready var ward = get_node(Ward)
onready var camera = get_node(Cam)
onready var inputs = get_node(Inputs) if Inputs else null


func _input(event):
	if ward is Player:
		var motion_direction = Vector2(
				Input.get_action_strength("move_right") - Input.get_action_strength("move_left"),
				Input.get_action_strength("move_back") - Input.get_action_strength("move_forward"))
		
		if motion_direction.length() > 0.0:
			ward.move(motion_direction)
			
		if motion_direction.length() < 0.001:
			ward.stop()
			
		if Input.is_action_just_pressed("jump"): #idle/move
			ward.jump()
		
		if Input.is_action_pressed("aim"): #idle/move
			ward.aim()
			
		if Input.is_action_pressed("shoot"): #idle/move
			ward.shoot()
	elif ward is Spacecraft:
		if Input.is_action_just_pressed("throttle"):
			ward.throttle(true)
		elif Input.is_action_just_released("throttle"):
			ward.throttle(false)
		
		var torque := Vector3.ZERO
		torque.x = Input.get_action_strength("pitch_up") - Input.get_action_strength("pitch_down")
		torque.y = Input.get_action_strength("yaw_right") - Input.get_action_strength("yaw_left")
		torque.z = Input.get_action_strength("roll_ccw") - Input.get_action_strength("roll_cw")
		
		print(torque)
		ward.change_orientation(torque)
#		if torque.length() > 0:
			
#		if : #dive up
#			add_torque(global_transform.basis.x * -THRUST_TURN * Z_FRONT)
#		if Input.is_action_pressed("pitch_down"): #dive down
#			add_torque(global_transform.basis.x * THRUST_TURN * Z_FRONT)
#		if Input.is_action_pressed("yaw_left"):
#			add_torque(global_transform.basis.y * THRUST_TURN * Z_FRONT)
#		if Input.is_action_pressed("yaw_right"):
#			add_torque(global_transform.basis.y * -THRUST_TURN * Z_FRONT)
#		if Input.is_action_pressed("roll_ccw"):
#			add_torque(global_transform.basis.z * -THRUST_ROLL * Z_FRONT)
#		if Input.is_action_pressed("roll_cw"):
#			add_torque(global_transform.basis.z * THRUST_ROLL * Z_FRONT)
