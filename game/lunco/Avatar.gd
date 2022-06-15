extends lnSpaceSystem

export (NodePath) var Player
export (NodePath) var Spacecraft
export (NodePath) var Operator

#-------------------------------
const MOUSE_SENSITIVITY = 0.1
const RAY_LENGTH = 10000

#-------------------------------

var ward: Node
var mouse_control := false

#-------------------------------

onready var ui := $UI
onready var state := $State
onready var matrix: lnMatrix = get_parent()
onready var camera := $SpringArmCamera

#-------------------------------

func set_ward(_ward):
	ward = _ward
	return ward

func set_camera(_camera):
	camera = _camera
	if camera:
		camera.set_current()

func set_ui(_ui):
	clear_ui()
	if(_ui):
		ui.add_child(_ui)
		
func clear_ui():
	for n in ui.get_children():
		ui.remove_child(n)
		n.queue_free()

#-------------------------------

# Input should scroll through all lnSystems. There should be a mapping between key and 
# object command
# Avatar should know this mapping
# Avatar has camera, that should be instantiated here
# And ward - object that is being controlled. In general 
# Avatar has it's own camera + world should have it's cameras.
# Avatar could list all cameras
# Avatar could list all objects. Some of them could be controlled e.g. command send
# Matrix itself is a space system and can perform commands
	
func _input(event):
	
	if event is InputEventMouseButton and event.pressed and event.button_index == 1:
		var e: InputEventMouseButton = event
		var position = e.position
		
		if camera:  
			var from = camera.project_ray_origin(position)
			var to = from + camera.project_ray_normal(position) * RAY_LENGTH	
			var res = matrix.ray_cast(from, to)
			if res:
				matrix.spawn(res["position"])
			
			
	if Input.is_action_just_pressed("select_player"):
		state.set_trigger("player")
	elif Input.is_action_just_pressed("select_spacecraft"):
		state.set_trigger("spacecraft")
	elif Input.is_action_just_pressed("select_operator"):
		state.set_trigger("operator")
		
	if Input.is_action_pressed("rotate_camera"):
		Input.set_mouse_mode(Input.MOUSE_MODE_CAPTURED)
		mouse_control = true
	else:
		Input.set_mouse_mode(Input.MOUSE_MODE_VISIBLE)
		mouse_control = false
	
	if camera is SpringArmCamera:
		var cam: SpringArmCamera = camera
		var camera_move := Vector2.ZERO
			
		if (event is InputEventMouseMotion) and mouse_control:
			camera_move = event.relative * MOUSE_SENSITIVITY
		else:
			camera_move = Vector2(
				Input.get_action_strength("camera_left") - Input.get_action_strength("camera_right"),
				Input.get_action_strength("camera_up") - Input.get_action_strength("camera_down")
			)
		
		var camera_spring_length = Input.get_action_strength("plus") - Input.get_action_strength("minus")
		
		cam.spring_length(camera_spring_length)
		
		if camera_move.length_squared() > 0.0:
			cam.rotate_relative(camera_move)
			
	match state.get_current():
		"Player":
			var player: Player = ward
			
			if not player:
				return
				
			var motion_direction := Vector3(
				Input.get_action_strength("move_right") - Input.get_action_strength("move_left"),
				Input.get_action_strength("move_up") - Input.get_action_strength("move_down"),
				Input.get_action_strength("move_back") - Input.get_action_strength("move_forward"))
		
			if motion_direction.length() < 0.001:
				player.stop()
			else:
				player.move(motion_direction)
				
			if Input.is_action_just_pressed("jump"): #idle/move
				player.jump()
			
#			if Input.is_action_pressed("aim"): #idle/move
#				player.aim()
				
			if Input.is_action_pressed("shoot"): #idle/move
				player.shoot()
			
			if camera is SpringArmCamera:
				var cam: SpringArmCamera = camera
				player.set_camera_x_rot(cam.camera_x_rot)
				player.set_camera_basis(cam.get_plain_basis())
				
		"Spacecraft":
			var spacecraft: Spacecraft = ward
			
			if Input.is_action_just_pressed("throttle"):
				spacecraft.throttle(true)
			elif Input.is_action_just_released("throttle"):
				spacecraft.throttle(false)
			
			var torque := Vector3(
				Input.get_action_strength("pitch_up") - Input.get_action_strength("pitch_down"),
				Input.get_action_strength("yaw_right") - Input.get_action_strength("yaw_left"),
				Input.get_action_strength("roll_cw") - Input.get_action_strength("roll_ccw")
			)
			
			spacecraft.change_orientation(torque)
			
		"Operator":
			var cam: SpringArmCamera = camera
			var operator: Operator = ward
			
			if Input.is_action_just_pressed("reset_position"):
				operator.reset_position();

			var motion_direction := Vector3(
				Input.get_action_strength("move_right") - Input.get_action_strength("move_left"),
				Input.get_action_strength("move_up") - Input.get_action_strength("move_down"),
				Input.get_action_strength("move_back") - Input.get_action_strength("move_forward")
			)

			operator.move(motion_direction)

func _on_State_transited(from, to):
	match to:
		"Player":
			set_ward(get_node(Player))
		"Spacecraft":
			set_ward(get_node(Spacecraft))
		"Operator":
			set_ward(get_node(Operator))
			
	camera.set_target(ward)

