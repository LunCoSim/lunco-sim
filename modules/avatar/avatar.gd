# Class lnAvatar which inherits from lnSpaceSystem
@icon("res://modules/avatar/avatar.svg")
class_name LCAvatar
extends LCSpaceSystem

# Declaring signals
signal create(path_to_scene)

signal create_operator
signal create_player
signal create_spacecraft

signal spawn_entity(entity, position)

signal ray_cast(from: Vector3, to: Vector3)

signal target_changed()

#-------------------------------
# Constants for mouse sensitivity and ray length
const MOUSE_SENSITIVITY = 0.015
const RAY_LENGTH = 10000

@export var MAX_SPEED = 100
@export var ACCELERATION = 50
@export var DECELERATION = 50

#var velocity := Vector3.ZERO
var dir := Vector3.ZERO
var orientation := Basis.IDENTITY

#-------------------------------
# Exporting target variable and setting default mouse control to false
@export var target: Node3D
var mouse_control := false

#-------------------------------
# Defining UI and camera variables
@onready var ui := $UI
@onready var camera := $SpringArmCamera

#------------------------------

@export var entity_to_spawn = EntitiesDB.Entities.Astronaut

#-------------------------------
# Function set_target sets the target, searches for a controller and calls state transited
func set_target(_target):
	if camera and target:
		camera.remove_excluded_object(target.get_parent())
		
	target = _target
	#searching for controller
	if _target: 
		#TBD: Better way to find controller
		for N in _target.get_children():
			if N is LCSpaceSystem:
				target = N
	
	if camera and target:
		camera.add_excluded_object(target.get_parent())
		
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
	set_target(target)
	set_camera(camera)
		
#-----------------------------------------------------

func action_raycast(position: Vector2):
	if camera:  
		var from = camera.project_ray_origin(position)
		var to = from + camera.project_ray_normal(position) * RAY_LENGTH
		emit_signal("ray_cast", from, to)
		
	
		var space_state = %Universe.get_world_3d().direct_space_state
		

		var query = PhysicsRayQueryParameters3D.create(from, to)
		query.exclude = [self]
		var result = space_state.intersect_ray(query)
		
		if result:
			if result.collider is StaticBody3D:
				spawn_entity.emit(entity_to_spawn, result.position + Vector3(0, 1, 0))
			else:
				set_target(result.collider)
				

func _input(event):
	if Input.is_action_just_pressed("click"):
		action_raycast(event.position) # TBD: Event could be different then expected
		
	if Input.is_action_just_pressed("ui_cancel"):
		#SceneManager.no_effect_change_scene("back")
		#TBD: Show/hide menu, should be a signal? To what?
		LCWindows.toggle_main_menu()
	
	# Creating entities
	if Input.is_action_just_pressed("select_player"):
		emit_signal("create_player")
	elif Input.is_action_just_pressed("select_spacecraft"):
		emit_signal("create_spacecraft")
	elif Input.is_action_just_pressed("select_operator"):
		emit_signal("create_operator")
	
	input_camera(event)
	input_character(event)
	input_operator(event)

func input_camera(event):
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
		
func input_character(event):
	if target is LCCharacterController:
		var character: LCCharacterController = target

		
		var motion = Vector2(
			Input.get_action_strength("move_right") - Input.get_action_strength("move_left"),
			Input.get_action_strength("move_back") - Input.get_action_strength("move_forward"))
			
		#character.motion = motion	
		#character.set_camera(camera)
		
func input_operator(event):
	if target is LCOperatorController:
		var cam: SpringArmCamera = camera
		var operator: LCOperatorController = target

		
		operator.orient(cam.get_plain_basis())
#------------------------------------------------------
# Function _on_State_transited instantiates different ui based on target and sets camera spring length
func _on_State_transited():

	camera.set_follow_height(0.5)
	camera.set_spring_length(2.5)
	
	if target is LCCharacterController:
		camera.set_spring_length(2.5)
		target.set_camera(camera) #TBD: Remove camera
	elif target is LCSpacecraftController:
		camera.set_spring_length(50)
		camera.set_follow_height(0)
	elif target is LCOperatorController:
		camera.set_spring_length(2.5)

	self.emit_signal("target_changed", target)

	ui.set_target(target)

	if camera != null:
		camera.target = target

func _on_select_entity_to_spawn(entity_id=0):
	entity_to_spawn = entity_id

func update_entities(entities):
	$UI.update_entities(entities)
# Function camera_global_position returns the global position of the camera
func camera_global_position():
	return camera.global_position
