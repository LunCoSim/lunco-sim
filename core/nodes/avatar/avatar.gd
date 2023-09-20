class_name lnAvatar
extends lnSpaceSystem


signal create(path_to_scene)

signal create_operator
signal create_player
signal create_spacecraft

signal ray_cast(from: Vector3, to: Vector3)

signal target_changed()

#-------------------------------
const MOUSE_SENSITIVITY = 0.015
const RAY_LENGTH = 10000

#-------------------------------

@export var target: Node3D
var mouse_control := false

#-------------------------------

@onready var ui := $UI/TargetUI
@onready var camera := $SpringArmCamera


#-------------------------------

func set_target(_target):
	target = _target
	
	if _target: #searching for controller
		for N in _target.get_children():
			if N is lnSpaceSystem:
				target = N

	_on_State_transited()
	return target

func set_camera(_camera):
	camera = _camera
	if camera:
		camera.set_current()

func set_ui(_ui=null):
	clear_ui()
	if(_ui):
		ui.add_child(_ui)
		_ui.set_target(target)
	
		
		
func clear_ui():
	if ui:
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
	
	print(target)
	set_target(target)
	set_camera(camera)
#	camera.set_target(target)
	

func _unhandled_input(event):
	
	#raycast

	#Left mouse button pressed
	if event is InputEventMouseButton and event.pressed and event.button_index == 1:
		print("Click mouse")
	
		print("Ray casting")
		
		var e: InputEventMouseButton = event
		var pos = e.position
		
		if camera:  
			var from = camera.project_ray_origin(pos)
			var to = from + camera.project_ray_normal(pos) * RAY_LENGTH
			
			emit_signal("ray_cast", from, to)	


func _input(event):
	if Input.is_action_just_pressed("ui_cancel"):
		SceneManager.no_effect_change_scene("back")
			
	if Input.is_action_just_pressed("select_player"):
		emit_signal("create_player")
		print("create_player")
	elif Input.is_action_just_pressed("select_spacecraft"):
		emit_signal("create_spacecraft")
	elif Input.is_action_just_pressed("select_operator"):
		emit_signal("create_operator")
		
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
		
		if event is InputEventMouseButton:
			if event.button_index == MOUSE_BUTTON_WHEEL_UP:
				print("Mouse wheel scrolled up!")
				camera_spring_length += -2
				
			elif event.button_index == MOUSE_BUTTON_WHEEL_DOWN:
				print("Mouse wheel scrolled down!")
				camera_spring_length += 2
		
		
		cam.spring_length(camera_spring_length)
		cam.rotate_relative(camera_move)
			
	
	if target is lnPlayer:
		var player: lnPlayer = target
		
		if not player:
			return
			
		var motion_direction := Vector3(
			Input.get_action_strength("move_right") - Input.get_action_strength("move_left"),
			Input.get_action_strength("move_up") - Input.get_action_strength("move_down"),
			Input.get_action_strength("move_back") - Input.get_action_strength("move_forward"))
	
#		if motion_direction.length() < 0.001:
#			player.stop()
#		else:
#			player.move(motion_direction)
#
#		if Input.is_action_just_pressed("jump"): #idle/move
#			player.jump()
#
##			if Input.is_action_pressed("aim"): #idle/move
##				player.aim()
#
#		if Input.is_action_pressed("shoot"): #idle/move
#			player.shoot()
		player.set_camera(camera)
#		if camera is SpringArmCamera:
#			var cam: SpringArmCamera = camera
#			player.set_camera_x_rot(cam.camera_x_rot)
#			player.set_camera_basis(cam.get_plain_basis())
				
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
		
		operator.move(motion_direction.normalized())
		operator.orient(cam.get_plain_basis())

func _on_State_transited():
	
	var _ui = null
	
	if target is lnPlayer:
		_ui = preload("res://core/ui/player-ui.tscn").instantiate()
#			camera.remove_excluded_object(Spacecraft)
		camera.set_spring_length(2.5)
		target.set_camera(camera)
	elif target is lnSpacecraft:
		_ui = preload("res://core/ui/spacecraft-ui.tscn").instantiate()
#			camera.add_excluded_object(Spacecraft)
		camera.set_spring_length(50)
	elif target is lnOperator:
		_ui = preload("res://core/ui/operator-ui.tscn").instantiate()
		_ui.model_selected.connect(_on_select_model)
		
#				camera.remove_excluded_object(Spacecraft)
		camera.set_spring_length(2.5)
	
	self.emit_signal("target_changed", target)
			
	set_ui(_ui)
	
	if camera != null:
		camera.target = target

func _on_select_model(path):
	print("_on_select_model: ", path)
#	spawn_model_path = path
