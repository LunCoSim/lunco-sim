# Class lnAvatar which inherits from lnSpaceSystem
class_name lnAvatar
extends lnSpaceSystem

# Declaring signals
signal create(path_to_scene)

signal create_operator
signal create_player
signal create_spacecraft

signal ray_cast(from: Vector3, to: Vector3)

signal target_changed()

#-------------------------------
# Constants for mouse sensitivity and ray length
const MOUSE_SENSITIVITY = 0.015
const RAY_LENGTH = 10000

#-------------------------------
# Exporting target variable and setting default mouse control to false
@export var target: Node3D
var mouse_control := false

#-------------------------------
# Defining UI and camera variables
@onready var ui := $UI
@onready var camera := $SpringArmCamera

#-------------------------------
# Function set_target sets the target, searches for a controller and calls state transited
func set_target(_target):
	if camera and target:
		camera.remove_excluded_object(target)
		
	target = _target
	#searching for controller
	if _target: 
		for N in _target.get_children():
			if N is lnSpaceSystem:
				target = N
	
	if camera and target:
		camera.add_excluded_object(target)
		
	# Calling state transited function
	_on_State_transited()
	return target

# Function set_camera sets the camera and make it current if camera exists
func set_camera(_camera):
	camera = _camera
	if camera:
		camera.set_current()



#-------------------------------
# Defining different functions for handling player controls like select, rotate, move, etc.
func _ready():

	print(target)
	set_target(target)
	set_camera(camera)



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
		#SceneManager.no_effect_change_scene("back")
		#TBD: Show/hide menu, should be a signal? To what?
		pass
	
	# Creating entities
	if Input.is_action_just_pressed("select_player"):
		emit_signal("create_player")
	elif Input.is_action_just_pressed("select_spacecraft"):
		emit_signal("create_spacecraft")
	elif Input.is_action_just_pressed("select_operator"):
		emit_signal("create_operator")
	
	# Rotating camera
	if Input.is_action_pressed("rotate_camera"):
		Input.set_mouse_mode(Input.MOUSE_MODE_CAPTURED)
		mouse_control = true
	else:
		Input.set_mouse_mode(Input.MOUSE_MODE_VISIBLE)
		mouse_control = false
		
	# Processing input related to camera
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


		cam.inc_spring_length(camera_spring_length)
		cam.rotate_relative(camera_move)


	if target is lnPlayer:
		var player: lnPlayer = target

		if not player:
			return

		var motion_direction := Vector3(
			Input.get_action_strength("move_right") - Input.get_action_strength("move_left"),
			Input.get_action_strength("move_up") - Input.get_action_strength("move_down"),
			Input.get_action_strength("move_back") - Input.get_action_strength("move_forward"))

		player.set_camera(camera)

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

# Function _on_State_transited instantiates different ui based on target and sets camera spring length
func _on_State_transited():

	camera.set_follow_height(0.5)
	camera.set_spring_length(2.5)
	
	if target is lnPlayer:
		camera.set_spring_length(2.5)
		target.set_camera(camera) #TBD: Remove camera
	elif target is lnSpacecraft:
		camera.set_spring_length(50)
		camera.set_follow_height(0)
	elif target is lnOperator:
		camera.set_spring_length(2.5)

	self.emit_signal("target_changed", target)

	ui.set_target(target)

	if camera != null:
		camera.target = target

# Function camera_global_position returns the global position of the camera
func camera_global_position():
	return camera.global_position
