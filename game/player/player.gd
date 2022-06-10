class_name Player
extends KinematicBody

export (NodePath) var camera

# Release aiming if the mouse/gamepad button was held for longer than 0.4 seconds.
# This works well for trackpads and is more accessible by not making long presses a requirement.
# If the aiming button was held for less than 0.4 seconds, keep aiming until the aiming button is pressed again.
const AIM_HOLD_THRESHOLD = 0.4

const DIRECTION_INTERPOLATE_SPEED = 1
const MOTION_INTERPOLATE_SPEED = 10
const ROTATION_INTERPOLATE_SPEED = 10

const MIN_AIRBORNE_TIME = 0.1
const JUMP_SPEED = 5

var airborne_time = 100

export var orientation = Transform()
export var root_motion = Transform()
export var motion = Vector2()
export var velocity = Vector3()

export var aiming = false

# If `true`, the aim button was toggled on by a short press (instead of being held down).
var toggled_aim = false

# The duration the aiming button was held for (in seconds).
var aiming_timer = 0.0


# Parameters
# strafe
# moving_direction
# moving_orientation
# state
# weapon_energy #droped to zero and then grows with time


# Commands:
# move(direction, orientation)
# jump()
# shoot()
# start_moving()
# stop()
# start_strafe
# set paramater
# set state
# set triggers

# State
# Idle
# Moving
# On_air
# Aiming
# Shooting


onready var initial_position = transform.origin
onready var gravity = ProjectSettings.get_setting("physics/3d/default_gravity") * ProjectSettings.get_setting("physics/3d/default_gravity_vector")

export onready var camera_node = get_node(camera)

onready var animation_tree = $AnimationTree
onready var player_model = $PlayerModel
onready var shoot_from = player_model.get_node(@"Robot_Skeleton/Skeleton/GunBone/ShootFrom")
onready var color_rect = $ColorRect
onready var crosshair = $Crosshair
onready var fire_cooldown = $FireCooldown

onready var sound_effects = $SoundEffects
onready var sound_effect_jump = sound_effects.get_node(@"Jump")
onready var sound_effect_land = sound_effects.get_node(@"Land")
onready var sound_effect_shoot = sound_effects.get_node(@"Shoot")

onready var smp = $StateMachinePlayer

var motion_target

func _ready():
	# Pre-initialize orientation transform.
	orientation = player_model.global_transform
	orientation.origin = Vector3()


func _physics_process(delta):
	
	motion_target = Vector2(
			Input.get_action_strength("move_right") - Input.get_action_strength("move_left"),
			Input.get_action_strength("move_back") - Input.get_action_strength("move_forward"))
	motion = motion.linear_interpolate(motion_target, MOTION_INTERPOLATE_SPEED * delta)

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
	
	camera_node.set_aiming(current_aim)

	# Jump/in-air logic.
	airborne_time += delta
	if is_on_floor():
		if airborne_time > 0.5:
			sound_effect_land.play()
		airborne_time = 0

	var on_air = airborne_time > MIN_AIRBORNE_TIME

	if not on_air and Input.is_action_just_pressed("jump"):
		velocity.y = JUMP_SPEED
		on_air = true
		# Increase airborne time so next frame on_air is still true
		airborne_time = MIN_AIRBORNE_TIME
		animation_tree["parameters/state/current"] = 2
		sound_effect_jump.play()

	if on_air:
		if (velocity.y > 0):
			animation_tree["parameters/state/current"] = 2
		else:
			animation_tree["parameters/state/current"] = 3
	elif aiming:
		# Change state to strafe.
		animation_tree["parameters/state/current"] = 0
		
		# Change aim according to camera rotation.
		if camera_node.camera_x_rot >= 0: # Aim up.
			animation_tree["parameters/aim/add_amount"] = -camera_node.camera_x_rot / deg2rad(camera_node.CAMERA_X_ROT_MAX)
		else: # Aim down.
			animation_tree["parameters/aim/add_amount"] = camera_node.camera_x_rot / deg2rad(camera_node.CAMERA_X_ROT_MIN)

		# Convert orientation to quaternions for interpolating rotation.
		var q_from = orientation.basis.get_rotation_quat()
		var q_to = camera_node.global_transform.basis.get_rotation_quat()
		# Interpolate current rotation with desired one.
		orientation.basis = Basis(q_from.slerp(q_to, delta * ROTATION_INTERPOLATE_SPEED))

		# The animation's forward/backward axis is reversed.
		animation_tree["parameters/strafe/blend_position"] = Vector2(motion.x, -motion.y)

		root_motion = animation_tree.get_root_motion_transform()

		if Input.is_action_pressed("shoot") and fire_cooldown.time_left == 0:
			var shoot_origin = shoot_from.global_transform.origin

			var ch_pos = crosshair.rect_position + crosshair.rect_size * 0.5
			var ray_from = camera_node.project_ray_origin(ch_pos)
			var ray_dir = camera_node.project_ray_normal(ch_pos)

			var shoot_target
			var col = get_world().direct_space_state.intersect_ray(ray_from, ray_from + ray_dir * 1000, [self], 0b11)
			if col.empty():
				shoot_target = ray_from + ray_dir * 1000
			else:
				shoot_target = col.position
			var shoot_dir = (shoot_target - shoot_origin).normalized()

			var bullet = preload("res://player/bullet/bullet.tscn").instance()
			get_parent().add_child(bullet)
			bullet.global_transform.origin = shoot_origin
			# If we don't rotate the bullets there is no useful way to control the particles ..
			bullet.look_at(shoot_origin + shoot_dir, Vector3.UP)
			bullet.add_collision_exception_with(self)
			var shoot_particle = $PlayerModel/Robot_Skeleton/Skeleton/GunBone/ShootFrom/ShootParticle
			shoot_particle.restart()
			shoot_particle.emitting = true
			var muzzle_particle = $PlayerModel/Robot_Skeleton/Skeleton/GunBone/ShootFrom/MuzzleFlash
			muzzle_particle.restart()
			muzzle_particle.emitting = true
			fire_cooldown.start()
			sound_effect_shoot.play()
			camera_node.add_camera_shake_trauma(0.35)

	else: # Not in air or aiming, idle.
		# Convert orientation to quaternions for interpolating rotation.
		var target = camera_node.camera_x * motion.x + camera_node.camera_z * motion.y
		if target.length() > 0.001:
			var q_from = orientation.basis.get_rotation_quat()
			var q_to = Transform().looking_at(target, Vector3.UP).basis.get_rotation_quat()
			# Interpolate current rotation with desired one.
			orientation.basis = Basis(q_from.slerp(q_to, delta * ROTATION_INTERPOLATE_SPEED))

		# Aim to zero (no aiming while walking).
		animation_tree["parameters/aim/add_amount"] = 0
		# Change state to walk.
		animation_tree["parameters/state/current"] = 1
		# Blend position for walk speed based on motion.
		animation_tree["parameters/walk/blend_position"] = Vector2(motion.length(), 0)

		root_motion = animation_tree.get_root_motion_transform()

	# Apply root motion to orientation.
	orientation *= root_motion

	var h_velocity = orientation.origin / delta
	velocity.x = h_velocity.x
	velocity.z = h_velocity.z
	velocity += gravity * delta
	velocity = move_and_slide(velocity, Vector3.UP)

	orientation.origin = Vector3() # Clear accumulated root motion displacement (was applied to speed).
	orientation = orientation.orthonormalized() # Orthonormalize orientation.

	player_model.global_transform.basis = orientation.basis


# --------------------------------------------------

func move(direction: Vector2, orientation: Vector3):
	smp.set_trigger("move")
	
func jump():
	smp.set_trigger("jump")
	
func shoot():
	smp.set_trigger("s")

# --------------------------------------------------
# Player state
func _on_StatePlayer_transited(from, to):
	pass # Replace with function body.


func _on_StatePlayer_updated(state, delta):
	pass # Replace with function body.

# Aiming
func _on_StateAiming_transited(from, to):
	pass # Replace with function body.


func _on_StateAiming_updated(state, delta):
	pass # Replace with function body.
