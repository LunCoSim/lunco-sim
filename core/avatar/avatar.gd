# Class lnAvatar which inherits from lnSpaceSystem
@icon("res://modules/avatar/avatar.svg")
class_name LCAvatar
extends LCSpaceSystem

#-------------------------------
# Declaring signals
signal create(path_to_scene)

signal create_operator
signal create_player
signal create_spacecraft

signal spawn_entity(entity, position)

signal target_changed(target)

#-------------------------------
# Constants for mouse sensitivity and ray length
const MOUSE_SENSITIVITY = 0.015
const RAY_LENGTH = 10000

#------------------------------------
# Block related to movement

@export var MAX_SPEED = 100
@export var ACCELERATION = 50
@export var DECELERATION = 50

#var velocity := Vector3.ZERO
var dir := Vector3.ZERO
var orientation := Basis.IDENTITY

#-------------------------------
# Exporting target variable and setting default mouse control to false
@export var target: Node3D
@export var entity_to_spawn = EntitiesDB.Entities.Astronaut
@export var selection: = []

#-------------------------------
# Defining UI and camera variables
@onready var ui := $UI
@onready var camera:SpringArmCamera = $SpringArmCamera

#------------------------------
# Internal state
var mouse_control := false

var UIs: = [] # TBD Global, e.g. at entity level. Each Entity has it's path to UI, Path to controller
var Controllers = [] # TBD Global

#-------------------------------
# Function set_target sets the target, searches for a controller and calls state transited
func set_target(_target):
	if camera and target:
		camera.remove_excluded_object(target.get_parent())
	
	if target is LCController:
		target.set_authority.rpc(1)
		#target.get_parent().set_multiplayer_authority(1)
		
	target = _target
	#searching for controller
	if _target: 
		#TBD: Better way to find controller
		for N in _target.get_children():
			if N is LCSpaceSystem:
				target = N
	
	if camera and target:
		camera.add_excluded_object(target.get_parent())
	
	if target is LCController:
		target.set_authority.rpc(multiplayer.get_unique_id())
		#target.get_parent().set_multiplayer_authority()
	# Calling state transited function
	_on_state_transited()
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

func action_raycast(_position: Vector2):
	if camera:  
		var from = camera.project_ray_origin(_position)
		var to = from + camera.project_ray_normal(_position) * RAY_LENGTH
		do_raycast(from, to)

func do_raycast(from: Vector3, to: Vector3):		
	var space_state = %Universe.get_world_3d().direct_space_state
	
	var query = PhysicsRayQueryParameters3D.create(from, to)
	query.exclude = [self]
	var result = space_state.intersect_ray(query)
	
	if result:
		#TBD Should be via tools, 3 tools: TargetTool, SelectionTool, SpawnTool
		#TBD Could be adding to selection
		if result.collider is StaticBody3D:
			spawn_entity.emit(entity_to_spawn, result.position + Vector3(0, 1, 0))
		else:
			set_target(result.collider)
				

func _input(event):
	#if Input.is_action_just_pressed("click"): #TBD Move to tools
		#action_raycast(event.position) # TBD: Event could be different then expected
		
	if Input.is_action_just_pressed("ui_cancel"): #TBD maybe move from avatar?
		#SceneManager.no_effect_change_scene("back")
		#TBD: Show/hide menu, should be a signal? To what?
		LCWindows.toggle_main_menu()
	
	if event is InputEventKey and not event.is_echo() and event.is_pressed():
		
		var key_number: int = -1
		
		match event.keycode:
			Key.KEY_1:
				key_number = 1
			Key.KEY_2:
				key_number = 2
			Key.KEY_3:
				key_number = 3
			Key.KEY_4:
				key_number = 4
			Key.KEY_5:
				key_number = 5
			Key.KEY_6:
				key_number = 6
			Key.KEY_7:
				key_number = 7
			Key.KEY_8:
				key_number = 8
		
		if key_number != -1:
			if event.is_alt_pressed():
				spawn_entity.emit(key_number-1)
			else:
				if get_parent().entities.size() >= key_number:
					set_target(get_parent().entities[key_number-1])
	
	input_camera(event)
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

		# Code responsible for camera movement. Better update to action
		if (event is InputEventMouseMotion) and mouse_control:
			camera_move = event.relative * MOUSE_SENSITIVITY
		
		camera_move += Vector2(
			Input.get_action_strength("camera_left") - Input.get_action_strength("camera_right"),
			Input.get_action_strength("camera_up") - Input.get_action_strength("camera_down")
		)
		cam.rotate_relative(camera_move) #-> For certain controllers new camera direction matters
		
		# Manupulations with camera. TBD to actions as well
		var delta_camera_spring_length = Input.get_action_strength("plus") - Input.get_action_strength("minus")

		if event is InputEventMouseButton:
			if event.button_index == MOUSE_BUTTON_WHEEL_UP:
				print("Mouse wheel scrolled up!")
				delta_camera_spring_length += -2

			elif event.button_index == MOUSE_BUTTON_WHEEL_DOWN:
				print("Mouse wheel scrolled down!")
				delta_camera_spring_length += 2
				
		cam.inc_spring_length(delta_camera_spring_length)
		
		
func input_operator(event):
	if target is LCOperatorController:
		var cam: SpringArmCamera = camera
		var operator: LCOperatorController = target

		
		operator.orient(cam.get_plain_basis())
#------------------------------------------------------
# Function _on_state_transited instantiates different ui based on target and sets camera spring length
func _on_state_transited():

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

	target_changed.emit(target)
	
	ui.set_target(target)

	if camera != null:
		camera.target = target

# Entities are tracked by simulation, and simulation sh
func update_entities(entities):
	$UI.update_entities(entities)
	
# Function camera_global_position returns the global position of the camera
func camera_global_position():
	return camera.global_position

#---------------------------------

func _on_select_entity_to_spawn(entity_id=0):
	entity_to_spawn = entity_id
	
func _on_ui_existing_entity_selected(index):
	set_target(get_parent().entities[index])
