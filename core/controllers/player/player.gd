class_name lnPlayer
extends CharacterBody3D

signal aiming

@export var camera: NodePath

# Release aiming if the mouse/gamepad button was held for longer than 0.4 seconds.
# This works well for trackpads and is more accessible by not making long presses a requirement.
# If the aiming button was held for less than 0.4 seconds, keep aiming until the aiming button is pressed again.
@export var AIM_HOLD_THRESHOLD = 0.4

@export var DIRECTION_INTERPOLATE_SPEED = 1
@export var MOTION_INTERPOLATE_SPEED = 100
@export var ROTATION_INTERPOLATE_SPEED = 10

@export var MIN_AIRBORNE_TIME = 0.1
@export var JUMP_SPEED = 5

#Used classes
const StateDirectory = preload("res://addons/imjp94.yafsm/src/StateDirectory.gd")

#------------------------------------------

var airborne_time = 100

@export var orientation = Transform3D()
@export var root_motion = Transform3D()

@export var motion := Vector3.ZERO
#@export var velocity := Vector3.ZERO
var motion_target := Vector3.ZERO

var camera_x_rot = 0.0
var camera_basis := Basis.IDENTITY

# Parameters
# strafe
# moving_direction
# moving_orientation
# state
# weapon_energy #droped to zero and then grows with time
# position
# orientation
# on_floor
# aiming


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


@onready var initial_position = transform.origin
@onready var gravity = ProjectSettings.get_setting("physics/3d/default_gravity") * ProjectSettings.get_setting("physics/3d/default_gravity_vector")

#----------------------------
#Connection with UI
#export onready var camera_node = get_node(camera)

@onready var animation_tree = $AnimationTree
@onready var player_model := $PlayerModel
#@onready var shoot_from = player_model.get_node("Robot_Skeleton/Skeleton/GunBone/ShootFrom")
@onready var color_rect = $ColorRect
@onready var crosshair = $Crosshair
@onready var fire_cooldown = $FireCooldown

#@onready var sound_effects = $SoundEffects
#@onready var sound_effect_jump = sound_effects.get_node("Jump")
#@onready var sound_effect_land = sound_effects.get_node("Land")
#@onready var sound_effect_shoot = sound_effects.get_node("Shoot")

@onready var state := $State

#----------------------------


func _ready():
	print("alksjdlksajdlkajdlk")
	# Pre-initialize orientation transform.
#	orientation = player_model.global_transform
#	orientation.origin = Vector3()
	pass
	print(state)
	
#
#
#func _process(delta):
#	#TBD: add tolerance timer + probably move to physics
#
#	$State.set_param("on_floor", is_on_floor())
#
#	match state.get_current():
#		"Idle":
#			pass
##			# Aim to zero (no aiming while walking).
##			animation_tree["parameters/aim/add_amount"] = 0
##			# Change state to walk.
##			animation_tree["parameters/state/current"] = 1
##			# Blend position for walk speed based on motion.
##			animation_tree["parameters/walk/blend_position"] = Vector2(motion.length(), 0)
#
#		"Move":
#			pass
#
#		"Jump":
#			velocity.y = JUMP_SPEED
#			state.set_trigger("on_air")
#			# Increase airborne time so next frame on_air is still true
##			animation_tree["parameters/state/current"] = 2
##			sound_effect_jump.play()
#		"On Air":
#			if (velocity.y > 0):
#				pass
##				animation_tree["parameters/state/current"] = 2
#			else:
#				pass
##				animation_tree["parameters/state/current"] = 3
#		"Landed":
##			sound_effect_land.play()
#			pass
#		"Aiming":
#			# Change state to strafe.
##			animation_tree["parameters/state/current"] = 0
#
##			# Change aim according to camera rotation.
##			if camera_x_rot >= 0: # Aim up.
##				animation_tree["parameters/aim/add_amount"] = -camera_x_rot / deg2rad(camera_node.CAMERA_X_ROT_MAX)
##			else: # Aim down.
##				animation_tree["parameters/aim/add_amount"] = camera_x_rot / deg2rad(camera_node.CAMERA_X_ROT_MIN)
##
##			# Convert orientation to quaternions for interpolating rotation.
##			var q_from = orientation.basis.get_rotation_quat()
##			var q_to = camera_node.get_rotation_quat()#TODO: remove somehow, this value should be updated only during aiming
##			# Interpolate current rotation with desired one.
##			orientation.basis = Basis(q_from.slerp(q_to, delta * ROTATION_INTERPOLATE_SPEED))
##
##			# The animation's forward/backward axis is reversed.
##			animation_tree["parameters/strafe/blend_position"] = Vector2(motion.x, -motion.y)
##
##			root_motion = animation_tree.get_root_motion_transform()
#			pass
#		"Shoot":
##			var shoot_origin = shoot_from.global_transform.origin
#
#			var ch_pos = crosshair.rect_position + crosshair.rect_size * 0.5
#			#TODO: project ray from Player's model
##			var ray_from = camera_node.project_ray_origin(ch_pos)
##			var ray_dir = camera_node.project_ray_normal(ch_pos)
##
##			var shoot_target
##			var col = get_world().direct_space_state.intersect_ray(ray_from, ray_from + ray_dir * 1000, [self], 0b11)
##			if col.empty():
##				shoot_target = ray_from + ray_dir * 1000
##			else:
##				shoot_target = col.position
##			var shoot_dir = (shoot_target - shoot_origin).normalized()
##
##			var bullet = preload("res://player/bullet/bullet.tscn").instance()
##			get_parent().add_child(bullet)
##			bullet.global_transform.origin = shoot_origin
##			# If we don't rotate the bullets there is no useful way to control the particles ..
##			bullet.look_at(shoot_origin + shoot_dir, Vector3.UP)
##			bullet.add_collision_exception_with(self)
##			var shoot_particle = $PlayerModel/Robot_Skeleton/Skeleton/GunBone/ShootFrom/ShootParticle
##			shoot_particle.restart()
##			shoot_particle.emitting = true
##			var muzzle_particle = $PlayerModel/Robot_Skeleton/Skeleton/GunBone/ShootFrom/MuzzleFlash
##			muzzle_particle.restart()
##			muzzle_particle.emitting = true
##			fire_cooldown.start()
##			sound_effect_shoot.play()
#
##			camera_node.add_camera_shake_trauma(0.35)# Todo: Emit signanl shot and connect to camera
#
##	root_motion = animation_tree.get_root_motion_position()
#	#rotating model to match movement direction
##	player_model.global_transform.basis = orientation.basis
#
#func _physics_process(delta):
#	# 
#	if motion_target.length_squared() > 0.00001:
#		motion = motion.lerp(motion_target, MOTION_INTERPOLATE_SPEED * delta)
#	else:
#		motion = motion_target
#
#	# Not in air or aiming, idle.
#	# Convert orientation to quaternions for interpolating rotation.
#	var target = camera_basis.x * motion.x + camera_basis.z * motion.z
#
#	if target.length_squared() > 0.001:
#		var q_from = orientation.basis.get_rotation_quaternion()
#		var q_to = Transform3D().looking_at(target, Vector3.UP).basis.get_rotation_quaternion()
#		# Interpolate current rotation with desired one.
#		orientation.basis = Basis(q_from.slerp(q_to, delta * ROTATION_INTERPOLATE_SPEED))
#
##	orientation.basis = camera_basis	
##	# Apply root motion to orientation.
#	orientation *= root_motion
#
#	var h_velocity = orientation.origin / delta
#	velocity.x = h_velocity.x
#	velocity.z = h_velocity.z
#
#	velocity += gravity * delta #TODO: Move to gravity vector
#
#	orientation.origin = Vector3() # Clear accumulated root motion displacement (was applied to speed).
#	orientation = orientation.orthonormalized() # Orthonormalize orientation.
#
##	velocity = move_and_slide(velocity, Vector3.UP)
#	move_and_slide()
#
#

# --------------------------------------------------
func start_move(direction: Vector2, orientation: Vector3):
	pass
	
func stop():
	if state:
		state.set_trigger("stop")
	
func move(direction: Vector3, orientation=null):
	if state:
		
		state.set_trigger("move")
		motion_target = direction
	
func jump():
	if state:
		state.set_trigger("jump")
	
func shoot():
	if state:
		state.set_trigger("shoot")

	
func aim():
	if state:
		state.set_trigger("aim")

func set_camera_x_rot(_camera_x_rot):
	camera_x_rot = _camera_x_rot

func set_camera_basis(basis: Basis):
	camera_basis = basis
	
# --------------------------------------------------
# Player state
func _on_StatePlayer_transited(from, to):
	var from_dir = StateDirectory.new(from)
	var to_dir = StateDirectory.new(to)
	print("On state transition from: %s to %s" % [from, to])
	
	match from:
		"Move":
#			motion_target = Vector2.ZERO
			print("From move")
		"Aiming":
#			camera_node.set_aiming(false)# TODO: should be state of camera
			pass
			
	match to:
		"Aiming":
#			camera_node.set_aiming(false)# TODO: should be state of camera			
#			camera_node.set_aiming(true)
			pass
		"Idle":
			motion_target = Vector3.ZERO

