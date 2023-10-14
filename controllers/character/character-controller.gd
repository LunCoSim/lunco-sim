@icon("res://controllers/player/character.svg")
## This controller represents character state and attaches to CharacterBody3D
class_name LCCharacterController
extends LCController

#---------------------------------------
signal land
signal jump
signal aiming_started
signal aiming_finished
signal shoot

#---------------------------------------
@export var character_body: LCCharacterBody

#----------------------------------------
const DIRECTION_INTERPOLATE_SPEED = 1
const MOTION_INTERPOLATE_SPEED = 10
const ROTATION_INTERPOLATE_SPEED = 10

const MIN_AIRBORNE_TIME = 0.1
const JUMP_SPEED = 5

var airborne_time = 100

var orientation = Transform3D()
var root_motion = Transform3D()
@export var motion = Vector2()

@onready var gravity = ProjectSettings.get_setting("physics/3d/default_gravity") * ProjectSettings.get_setting("physics/3d/default_gravity_vector")

#----------------------------------------

var aim_rotation
@export var input_motion: = Vector2.ZERO
var camera_rotation_bases: Basis = Basis.IDENTITY
var camera_base_quaternion: Quaternion = Quaternion.IDENTITY

var jumping: bool = false
@export var shooting: bool = false
@export var aiming: bool = false
@export var shoot_target: = Vector3.ZERO
@export var on_air: = false

#-------------------------------------

func _ready():
	if character_body == null:
		character_body = get_parent()

func _physics_process(delta: float):	
	if is_multiplayer_authority():
		apply_input(delta)
		
func apply_input(delta: float):
	if character_body == null:
		return
		
	motion = motion.lerp(input_motion, MOTION_INTERPOLATE_SPEED * delta)

	var camera_basis : Basis = camera_rotation_bases
	
	var camera_z := camera_basis.z
	var camera_x := camera_basis.x

	camera_z.y = 0
	camera_z = camera_z.normalized()
	camera_x.y = 0
	camera_x = camera_x.normalized()

	# Jump/in-air logic.
	airborne_time += delta
	if character_body.is_on_floor():
		if airborne_time > 0.5:
			land.emit()
		airborne_time = 0

	on_air = airborne_time > MIN_AIRBORNE_TIME

	if not on_air and jumping:
		character_body.velocity.y = JUMP_SPEED
		on_air = true
		# Increase airborne time so next frame on_air is still true
		airborne_time = MIN_AIRBORNE_TIME
		jump.emit()

	jumping = false

	if on_air:
		pass
	elif aiming:
		# Convert orientation to quaternions for interpolating rotation.
		var q_from = orientation.basis.get_rotation_quaternion()
		var q_to = camera_base_quaternion
		# Interpolate current rotation with desired one.
		orientation.basis = Basis(q_from.slerp(q_to, delta * ROTATION_INTERPOLATE_SPEED))

		root_motion = Transform3D(character_body.animation_tree.get_root_motion_rotation(), character_body.animation_tree.get_root_motion_position())

		if shooting and character_body.fire_cooldown.time_left == 0:
			var shoot_origin = character_body.shoot_from.global_transform.origin
			var shoot_dir = (shoot_target - shoot_origin).normalized()

			var bullet = preload("res://content/gobot/bullet/bullet.tscn").instantiate()
			get_parent().add_child(bullet, true)
			bullet.global_transform.origin = shoot_origin
			# If we don't rotate the bullets there is no useful way to control the particles ..
			bullet.look_at(shoot_origin + shoot_dir, Vector3.UP)
			bullet.add_collision_exception_with(self)
			
			shoot.emit()

	else: # Not in air or aiming, idle.
		# Convert orientation to quaternions for interpolating rotation.
		var target = camera_x * motion.x + camera_z * motion.y
		if target.length() > 0.001:
			var q_from = orientation.basis.get_rotation_quaternion()
			var q_to = Transform3D().looking_at(target, Vector3.UP).basis.get_rotation_quaternion()
			# Interpolate current rotation with desired one.
			orientation.basis = Basis(q_from.slerp(q_to, delta * ROTATION_INTERPOLATE_SPEED))

		root_motion = Transform3D(character_body.animation_tree.get_root_motion_rotation(), character_body.animation_tree.get_root_motion_position())

	# Apply root motion to orientation.
	orientation *= root_motion #????? What's happening here?
	do_move(delta)
	orientation.origin = Vector3() # Clear accumulated root motion displacement (was applied to speed).
	orientation = orientation.orthonormalized() # Orthonormalize orientation.
	

func do_move(delta):
	var h_velocity = orientation.origin / delta
	character_body.velocity.x = h_velocity.x
	character_body.velocity.z = h_velocity.z
	character_body.velocity += gravity * delta
	#character_body.set_velocity(character_body.velocity)
	character_body.set_up_direction(Vector3.UP)
	character_body.move_and_slide()
