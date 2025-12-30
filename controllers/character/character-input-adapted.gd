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

# Synchronized controls - kept for compatibility if needed, but driven by commands now
@export var aiming := false
@export var shoot_target := Vector3()
@export var motion := Vector2()
@export var shooting := false
@export var jumping := false

# Camera and effects
@export var camera_animation : AnimationPlayer
@export var crosshair : TextureRect

@export var camera : Node3D

@export var color_rect : ColorRect

# Previous state for change detection
var _prev_move_vector := Vector3.ZERO
var _prev_view_quat := Quaternion.IDENTITY
var _prev_aiming := false
var _prev_shooting := false

func _ready():
	if target == null:
		target = get_parent()

func _process(delta):
	var _target = get_resolved_target()

	if not _target is LCCharacterController:
		return
	
	# Only process input if we have authority over the character controller
	if not _target.has_authority():
		return
		
	# Check if input captured by UI
	var can_process = should_process_input()
	if not can_process:
		if _prev_move_vector != Vector3.ZERO:
			_prev_move_vector = Vector3.ZERO
			_send_command("SET_MOVE_VECTOR", {"x": 0, "y": 0, "z": 0})
		if _prev_aiming:
			_prev_aiming = false
			_send_command("SET_AIMING", {"is_aiming": false})
		if _prev_shooting:
			_prev_shooting = false
			_send_command("SET_SHOOTING", {"is_shooting": false})
		return
	
	# --- Motion ---
	var input_vec = Vector2(
			Input.get_action_strength("move_right") - Input.get_action_strength("move_left"),
			Input.get_action_strength("move_back") - Input.get_action_strength("move_forward"))
	
	# Calculate world move vector based on camera
	var camera_basis = get_camera_rotation_basis()
	var camera_z = camera_basis.z
	var camera_x = camera_basis.x
	
	camera_z.y = 0
	camera_z = camera_z.normalized()
	camera_x.y = 0
	camera_x = camera_x.normalized()
	
	var move_vec_world = (camera_x * input_vec.x + camera_z * input_vec.y)
	
	# Clamp length
	if input_vec.length() > 1.0:
		move_vec_world = move_vec_world.normalized()
	
	if not move_vec_world.is_equal_approx(_prev_move_vector):
		_prev_move_vector = move_vec_world
		_send_command("SET_MOVE_VECTOR", {"x": move_vec_world.x, "y": move_vec_world.y, "z": move_vec_world.z})
	
	# --- View / Aiming Rotation ---
	var view_quat = get_camera_base_quaternion()
	if abs(view_quat.dot(_prev_view_quat)) < 0.9999:
		_prev_view_quat = view_quat
		_send_command("SET_VIEW_QUATERNION", {"x": view_quat.x, "y": view_quat.y, "z": view_quat.z, "w": view_quat.w})

	# --- Aiming Logic ---
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
	
	if aiming != _prev_aiming:
		_prev_aiming = aiming
		_send_command("SET_AIMING", {"is_aiming": aiming})

	# --- Jumping ---
	if Input.is_action_just_pressed("jump"):
		_send_command("JUMP", {})

	# --- Shooting ---
	shooting = Input.is_action_pressed("shoot")
	if shooting != _prev_shooting:
		_prev_shooting = shooting
		_send_command("SET_SHOOTING", {"is_shooting": shooting})

# Helper to send commands
func _send_command(cmd_name: String, args: Dictionary):
	var _target = get_resolved_target()
	if not _target: return
	
	var cmd = LCCommand.new(cmd_name, _target.get_path(), args, "local")
	LCCommandRouter.dispatch(cmd)

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
