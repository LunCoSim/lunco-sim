extends Node

export (NodePath) var PlayerCam #camera
export (NodePath) var SpacecraftCam
export (NodePath) var OperatorCam

export (NodePath) var Player
export (NodePath) var Spacecraft
export (NodePath) var Operator

export (NodePath) var OperatorUI
export (NodePath) var SpacecarftUI
export (NodePath) var PlayerUI

export (NodePath) var Inputs

onready var ward: Node
onready var camera: Node

onready var ui = $"UI"
onready var spacecraft_ui = get_node(SpacecarftUI)
onready var inputs = get_node(Inputs) if Inputs else null

onready var state := $State

func set_ward(_ward):
	ward = _ward

func set_camera(_camera):
	camera = _camera
	camera.set_current()

func set_ui(_ui):
	clear_ui()
	if(_ui):
		ui.add_child(_ui)

func _input(event):
	if Input.is_action_just_pressed("select_player"):
		state.set_trigger("player")
	elif Input.is_action_just_pressed("select_spacecraft"):
		state.set_trigger("spacecraft")
	elif Input.is_action_just_pressed("select_operator"):
		state.set_trigger("operator")
		
	match state.get_current():
		"Player":
			var motion_direction = Vector2(
				Input.get_action_strength("move_right") - Input.get_action_strength("move_left"),
				Input.get_action_strength("move_back") - Input.get_action_strength("move_forward"))
		
			if motion_direction.length() > 0.0:
				ward.move(motion_direction)
			elif motion_direction.length() < 0.001:
				ward.stop()
				
			if Input.is_action_just_pressed("jump"): #idle/move
				ward.jump()
			
			if Input.is_action_pressed("aim"): #idle/move
				ward.aim()
				
			if Input.is_action_pressed("shoot"): #idle/move
				ward.shoot()
		"Spacecraft":
			if Input.is_action_just_pressed("throttle"):
				ward.throttle(true)
			elif Input.is_action_just_released("throttle"):
				ward.throttle(false)
			
			var torque := Vector3.ZERO
			torque.x = Input.get_action_strength("pitch_up") - Input.get_action_strength("pitch_down")
			torque.y = Input.get_action_strength("yaw_right") - Input.get_action_strength("yaw_left")
			torque.z = Input.get_action_strength("roll_ccw") - Input.get_action_strength("roll_cw")
			
			ward.change_orientation(torque)
		"Operator":
			var operator: Operator = ward
			if Input.is_action_just_pressed("reset_position"):
				operator.reset_position();
#			
			var dir := Vector3.ZERO
			dir.x = Input.get_action_strength("move_right") - Input.get_action_strength("move_left")
			dir.z = Input.get_action_strength("move_back") - Input.get_action_strength("move_forward")	
			dir.y = Input.get_action_strength("move_up") - Input.get_action_strength("move_down")	
			operator.move(dir)

func clear_ui():
	for n in ui.get_children():
		ui.remove_child(n)
		n.queue_free()
				
func _on_State_transited(from, to):
	match to:
		"Player":
			set_ward(get_node(Player))
			set_camera(get_node(PlayerCam))
			set_ui(PlayerUI)
		"Spacecraft":
			set_ward(get_node(Spacecraft))
			set_camera(get_node(SpacecraftCam))
			set_ui(spacecraft_ui)
		"Operator":
			set_ward(get_node(Operator))
			set_camera(get_node(OperatorCam))
			set_ui(OperatorUI)
