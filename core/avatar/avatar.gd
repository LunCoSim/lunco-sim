# Class lnAvatar which inherits from lnSpaceSystem
@icon("res://core/avatar/avatar.svg")
class_name LCAvatar
extends LCSpaceSystem

#-------------------------------
# Declaring signals
signal create(path_to_scene)

signal spawn_entity(entity, position)

signal target_changed(target)

signal requesting_control(entity_idx)
signal release_control

#-------------------------------
# Constants for mouse sensitivity and ray length
const MOUSE_SENSITIVITY = 0.015
const RAY_LENGTH = 10000

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
var controller: LCController

var UIs: = [] # TBD Global, e.g. at entity level. Each Entity has it's path to UI, Path to controller
var Controllers = [] # TBD Global

#-------------------------------
# Function set_target sets the target, searches for a controller and calls state transited
func set_target(_target):
	print("Set target: ", _target)
	if camera and target:
		camera.remove_excluded_object(controller.get_parent())
	
	if controller:
		release_control.emit(controller.get_parent().get_path())
	
	target = _target
	#searching for controller
	if not _target is LCController: 
		#TBD: Better way to find controller
		target = LCController.find_controller(_target)
		controller = LCController.find_controller(_target)
	
	if camera and controller:
		camera.add_excluded_object(controller.get_parent())
	
	# Calling state transited function
	_on_state_transited()
	return controller

# Function set_camera sets the camera and make it current if camera exists
func set_camera(_camera):
	camera = _camera
	if camera:
		camera.set_current()

#-------------------------------
# Defining different functions for handling player controls like select, rotate, move, etc.
func _ready():
	set_camera(camera)
	set_target(target)
	ControlManager.control_granted.connect(_on_control_granted)
	ControlManager.control_request_denied.connect(_on_control_request_denied)

#-----------------------------------------------------

# Add these new constants
const NFT_SPHERE_SCENE = preload("res://core/facilities/nft-sphere.tscn")
const POPUP_SCENE = preload("res://core/widgets/nft-create-popup.tscn")

# Add this as a class variable
var active_popup: Control = null

# Modify the spawn_nft_sphere function to use RPC
@rpc("any_peer", "call_local")
func spawn_nft_sphere(nft_data: Dictionary, position: Vector3):
	var nft_sphere = NFT_SPHERE_SCENE.instantiate()
	nft_sphere.set_nft_data(nft_data)
	nft_sphere.global_transform.origin = position + Vector3(0, 1, 0)  # Offset slightly above the ground
	%Universe.add_child(nft_sphere)
	print("Spawned NFT sphere at position: ", nft_sphere.global_transform.origin)
	print("NFT data set: ", nft_data)  # Debug print

# Modify the _on_nft_issued function to use RPC
func _on_nft_issued(nft_data, position: Vector3):
	print("NFT issued with data: ", nft_data)  # Debug print
	if multiplayer.is_server():
		spawn_nft_sphere.rpc(nft_data, position)
	else:
		# Send to server for validation and distribution
		spawn_nft_sphere.rpc_id(1, nft_data, position)
	active_popup.queue_free()
	active_popup = null  # Clear the active popup reference

# Add this new function
func handle_click(event_position: Vector2):
	# First, check if we clicked on the existing popup
	if active_popup and active_popup.get_global_rect().has_point(event_position):
		return  # Ignore clicks on the popup itself

	if camera:
		var from = camera.project_ray_origin(event_position)
		var to = from + camera.project_ray_normal(event_position) * RAY_LENGTH
		var result = do_raycast_nft(from, to)
		
		if result and result.collider is StaticBody3D:
			if Input.is_key_pressed(KEY_CTRL):  # Check if Ctrl is pressed
				if not active_popup:  # Only create a new popup if one doesn't exist
					show_nft_popup(result.position)

			# if Profile.wallet != "":  # Assuming you have a Global singleton to check login status
			# 	if not active_popup:  # Only create a new popup if one doesn't exist
			# 		show_nft_popup(result.position)
			# else:
			# 	print("Please log in with Web3 wallet first")

# Add this new function (renamed from do_raycast to do_raycast_nft)
func do_raycast_nft(from: Vector3, to: Vector3):        
	var space_state = %Universe.get_world_3d().direct_space_state
	
	var query = PhysicsRayQueryParameters3D.create(from, to)
	query.exclude = [self]
	return space_state.intersect_ray(query)

# Add these new functions
func show_nft_popup(position: Vector3):
	active_popup = POPUP_SCENE.instantiate()
	add_child(active_popup)

	active_popup.connect("nft_issued", Callable(self, "_on_nft_issued").bind(position))
	active_popup.connect("tree_exited", Callable(self, "_on_popup_closed"))

func _on_popup_closed():
	active_popup = null  # Clear the active popup reference when it's closed

# Modify the _input function to use the new handle_click function
func _input(event):
	if active_popup:
		return  # Ignore input when popup is active

	if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT and event.pressed:
		handle_click(event.position)

	if Input.is_action_just_pressed("ui_cancel"): #TBD maybe move from avatar?
		#SceneManager.no_effect_change_scene("back")
		#TBD: Show/hide menu, should be a signal? To what?
		LCWindows.toggle_main_menu()
	
	if Input.is_action_just_pressed("ui_focus_next"):
		LCWindows.toggle_chat()
	
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
			Key.KEY_BACKSPACE:
				set_target(null)
				return
				
		if key_number != -1:
			if event.is_alt_pressed():
				spawn_entity.emit(key_number-1)
			else:
				requesting_control.emit(key_number-1)
		
	if target == null:
		var motion_direction := Vector3(
			Input.get_action_strength("move_left") - Input.get_action_strength("move_right"),
			Input.get_action_strength("move_up") - Input.get_action_strength("move_down"),
			Input.get_action_strength("move_forward") - Input.get_action_strength("move_back")
		)
		$AvatarController.direction = motion_direction
		$AvatarController.camera_basis = camera.get_camera_rotation_basis()
		
		if Input.is_key_pressed(KEY_ALT):
			$AvatarController.speed = 100
		elif Input.is_key_pressed(KEY_SHIFT):
			$AvatarController.speed = 20
		else:
			$AvatarController.speed = 10
	else:
		$AvatarController.direction = Vector3.ZERO
		
		
		
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
				delta_camera_spring_length += -2

			elif event.button_index == MOUSE_BUTTON_WHEEL_DOWN:
				delta_camera_spring_length += 2
				
		cam.inc_spring_length(delta_camera_spring_length)
		
		
func input_operator(event):
	if target is LCOperatorController:
		var operator: LCOperatorController = target
		operator.orient(camera.get_plain_basis())
		
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
			requesting_control.emit(result.collider)
#------------------------------------------------------
# Function _on_state_transited instantiates different ui based on target and sets camera spring length
func _on_state_transited():

	camera.set_follow_height(0.5)
	camera.set_spring_length(2.5)
	
	if target is LCCharacterController:
		camera.set_spring_length(2.5)
		camera.set_follow_height(2.0)
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

var controlled_entities = []

func _on_select_entity_to_spawn(entity_id=0, position=null):
	if is_multiplayer_authority():
		get_parent().spawn.rpc_id(1, entity_id, position)
	else:
		get_parent().spawn.rpc_id(1, entity_id, position)

func _on_existing_entity_selected(idx):
	print("Avatar: Requesting control for entity index: ", idx)
	get_parent().request_control_by_index(idx)

func request_release_control():
	if target:
		get_parent()._on_avatar_release_control(target.get_path())
		set_target(null)

func _on_simulation_control_granted(path):
	print("Avatar: Control granted for entity: ", path)
	var entity = get_node(path)
	if entity:
		print("Avatar: Setting target to ", entity.name)
		set_target(entity)
	else:
		print("Avatar: Failed to get node for path: ", path)

func _on_simulation_control_declined(path):
	print("Avatar: Control declined for entity: ", path)

func _on_simulation_control_released(path):
	print("Avatar: Control released for entity: ", path)

func _on_select_entity_to_control(entity):
	if entity is Node:  # Ensure entity is a Node
		ControlManager.request_control(entity.get_path())
	else:
		print("Error: entity is not a Node")

func _on_release_control(entity):
	if entity is Node:  # Ensure entity is a Node
		ControlManager.release_control(entity.get_path())
	else:
		print("Error: entity is not a Node")

func _on_control_granted(peer_id: int, entity_path: NodePath):
	if peer_id == multiplayer.get_unique_id():
		print("Avatar: Control granted for entity: ", entity_path)
		var entity = get_node(entity_path)
		set_target(entity)
		# Update UI or other necessary changes

func _on_control_request_denied(peer_id: int, entity_path: NodePath):
	if peer_id == multiplayer.get_unique_id():
		print("Avatar: Control denied for entity: ", entity_path)
		# Update UI or show a message to the user
