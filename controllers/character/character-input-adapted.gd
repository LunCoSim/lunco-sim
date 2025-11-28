class_name LCCharacterInputAdapter
extends LCInputAdapter

#------------------------------
# target is inherited from LCInputAdapter

#------------------------------
const CAMERA_CONTROLLER_ROTATION_SPEED := 3.0
const CAMERA_MOUSE_ROTATION_SPEED := 0.001
# A minimum angle lower than or equal to -90 breaks movement if the player is looking upward.
const CAMERA_X_ROT_MIN := deg_to_rad(-89.9)
const CAMERA_X_ROT_MAX := deg_to_rad(70)

# Release aiming if the mouse/gamepad button was held for longer than 0.4 seconds.
# This works well for trackpads and is more accessible by not making long presses a requirement.
# If the aiming button was held for less than 0.4 seconds, keep aiming until the aiming button is pressed again.
const AIM_HOLD_THRESHOLD = 0.4

# If `true`, the aim button was toggled checked by a short press (instead of being held down).
var toggled_aim := false

# The duration the aiming button was held for (in seconds).
var aiming_timer := 0.0

# Synchronized controls
@export var aiming := false
@export var shoot_target := Vector3()
@export var motion := Vector2()
@export var shooting := false
# This is handled via RPC for now
@export var jumping := false

# Camera and effects
@export var camera_animation : AnimationPlayer
@export var crosshair : TextureRect

@export var camera : Node3D

@export var color_rect : ColorRect


func _ready():
	if target == null:
		target = get_parent()
		
	#if get_multiplayer_authority() == multiplayer.get_unique_id():
		#Logger.info("plaer autority multiplayer, on: ", multiplayer.get_unique_id())
##		camera_camera.make_current()
		#Input.set_mouse_mode(Input.MOUSE_MODE_CAPTURED)
	#else:
		#print("plaer autority reqular, on: ", multiplayer.get_unique_id())
##		set_process(false)
##		set_process_input(false)
		#color_rect.hide()

func _process(delta):
	var _target = get_resolved_target()

	
	if not _target is LCCharacterController:
		return
	
	# Only process input if we have authority over the character controller
	if not _target.has_authority():
		return
		
	# Check if input is captured by UI
	if not should_process_input():
		_target.input_motion = Vector2.ZERO
		_target.aiming = false
		_target.shooting = false
		return
	
	motion = Vector2(
			Input.get_action_strength("move_right") - Input.get_action_strength("move_left"),
			Input.get_action_strength("move_back") - Input.get_action_strength("move_forward"))
	
	# Setting Gobot parameters
	_target.input_motion = motion
	_target.camera_rotation_bases = get_camera_rotation_basis()
	_target.camera_base_quaternion =get_camera_base_quaternion()
	#--------------
	
	var current_aim = false

	# Keep aiming if the mouse wasn't held for long enough.
	if Input.is_action_just_released("aim") and aiming_timer <= AIM_HOLD_THRESHOLD:
		current_aim = true
		toggled_aim = true
	else:
		current_aim = toggled_aim or Input.is_action_pressed("aim")
		if Input.is_action_just_pressed("aim"):
			toggled_aim = false

	if current_aim:
		aiming_timer += delta
	else:
		aiming_timer = 0.0

	if aiming != current_aim:
		aiming = current_aim
		_target.aiming = aiming
#		if aiming:
#			camera_animation.play("shoot")
#		else:
#			camera_animation.play("far")

	if Input.is_action_just_pressed("jump"):
		jump.rpc()

	shooting = Input.is_action_pressed("shoot")
	_target.shooting = shooting
	
	if shooting:
		pass
#		var ch_pos = crosshair.position + crosshair.size * 0.5
#		var ray_from = camera_camera.project_ray_origin(ch_pos)
#		var ray_dir = camera_camera.project_ray_normal(ch_pos)
#
#		var col = get_parent().get_world_3d().direct_space_state.intersect_ray(PhysicsRayQueryParameters3D.create(ray_from, ray_from + ray_dir * 1000, 0b11, [self]))
#		if col.is_empty():
#			shoot_target = ray_from + ray_dir * 1000
#		else:
#			shoot_target = col.position

	# Fade out to black if falling out of the map. -17 is lower than
	# the lowest valid position checked the map (which is a bit under -16).
	# At 15 units below -17 (so -32), the screen turns fully black.
#	var tr : Transform3D = get_parent().global_transform
#	if tr.origin.y < -17:
#		color_rect.modulate.a = min((-17 - tr.origin.y) / 15, 1)
#	else:
#		# Fade out the black ColorRect progressively after being teleported back.
#		color_rect.modulate.a *= 1.0 - delta * 4
	_target.aim_rotation = get_aim_rotation()

func get_aim_rotation():
	var x := 0.0
	
	if camera:
		x = camera.get_camera_x_rot()
		
	var camera_x_rot = clamp(x, CAMERA_X_ROT_MIN, CAMERA_X_ROT_MAX)
	# Change aim according to camera rotation.
	if camera_x_rot >= 0: # Aim up.
		return -camera_x_rot / CAMERA_X_ROT_MAX
	else: # Aim down.
		return camera_x_rot / CAMERA_X_ROT_MIN


func get_camera_base_quaternion() -> Quaternion:
	if camera:
		return camera.get_camera_base_quaternion()
	else: 
		return Quaternion.IDENTITY


func get_camera_rotation_basis() -> Basis:
	if camera:
		return camera.get_camera_rotation_basis()
	else:
		return Basis.IDENTITY


func set_camera(_camera):
	camera = _camera
	
@rpc("any_peer", "call_local")
func jump():
	var _target = get_resolved_target()
	
	if not _target is LCCharacterController:
		return
		
	_target.jumping = true
