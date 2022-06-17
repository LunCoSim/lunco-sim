extends lnSpaceSystem

#-------------------------------
const MOUSE_SENSITIVITY = 0.1
const RAY_LENGTH = 10000

#-------------------------------

var target: Node
var mouse_control := false

var player
var spacecraft
var operator

var spawn_model_path = "res://addons/lunco-content/moonwards/buildings/android-kiosk/android-kiosk.escn"

#-------------------------------

onready var ui := $UI/TargetUI
onready var state := $State
onready var matrix: lnMatrix = get_parent()
onready var camera := $SpringArmCamera

#-------------------------------

func set_target(_target):
	target = _target
	return target

func set_camera(_camera):
	camera = _camera
	if camera:
		camera.set_current()

func set_ui(_ui=null):
	clear_ui()
	if(_ui):
		ui.add_child(_ui)
		
func clear_ui():
	for n in ui.get_children():
		ui.remove_child(n)

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

func _ready():
	player = matrix.get_player()
	spacecraft = matrix.get_spacecraft()
	operator = matrix.get_operator()

func _unhandled_input(event):
	if target is lnOperator:
		if event is InputEventMouseButton and event.pressed and event.button_index == 1:
			var e: InputEventMouseButton = event
			var position = e.position
			
			if camera:  
				var from = camera.project_ray_origin(position)
				var to = from + camera.project_ray_normal(position) * RAY_LENGTH	
				var res = matrix.ray_cast(from, to)
				if res:
					matrix.spawn(res["position"], spawn_model_path)
					
func _input(event):
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
	
	if target is lnPlayer:
		var player: lnPlayer = target
		
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
				
	elif target is lnSpacecraft:
		var spacecraft: lnSpacecraft = target
		
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
			
	elif target is lnOperator:
		var cam: SpringArmCamera = camera
		var operator: lnOperator = target
		
		if Input.is_action_just_pressed("reset_position"):
			operator.reset_position();

		var motion_direction := Vector3(
			Input.get_action_strength("move_left") - Input.get_action_strength("move_right"),
			Input.get_action_strength("move_up") - Input.get_action_strength("move_down"),
			Input.get_action_strength("move_forward") - Input.get_action_strength("move_back")
		)

		operator.move(motion_direction)
		operator.orient(cam.get_plain_basis())

func _on_State_transited(from, to):
	var _ui = null
	match to:
		"Player":
			set_target(player)
			_ui = preload("res://ui/player-ui.tscn").instance()
			$UI/Target.text = "Target: Player"
		"Spacecraft":
			set_target(spacecraft)
			_ui = preload("res://ui/spacecraft-ui.tscn").instance()
			$UI/Target.text = "Target: Spacecraft"
		"Operator":
			set_target(operator)
			_ui = preload("res://ui/operator-ui.tscn").instance()
			_ui.connect("model_selected", self, "_on_select_model")
			$UI/Target.text = "Target: Operator"
			
	set_ui(_ui)
	if _ui:
		_ui.set_target(target)
	camera.set_target(target)

func _on_select_model(path):
	print("_on_select_model: ", path)
	spawn_model_path = path
