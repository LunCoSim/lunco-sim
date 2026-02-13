@icon("res://controllers/character/character.svg")
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

@export var orientation: = Transform3D()
var root_motion = Transform3D()
@export var motion = Vector2()

@onready var gravity = ProjectSettings.get_setting("physics/3d/default_gravity") * ProjectSettings.get_setting("physics/3d/default_gravity_vector")

@export var use_root_motion: bool = true
@export var move_speed: float = 5.0

#----------------------------------------

# State variables (set via commands)
var move_vector := Vector3.ZERO
var view_quaternion := Quaternion.IDENTITY
@export var input_motion = Vector2() # Raw input synced from client
# input_motion is no longer used for direction calculation, but kept for legacy sync and potential animation speed blend.
# actually we can just use move_vector length.

# Restored State Variables
var jumping: bool = false
@export var shooting: bool = false
@export var aiming: bool = false
@export var shoot_target: = Vector3.ZERO
@export var on_air: = false

func _ready():
	if character_body == null:
		character_body = get_parent()
		
	# Add command executor
	var executor = LCCommandExecutor.new()
	executor.name = "CommandExecutor"
	add_child(executor)
	
	# Default view
	view_quaternion = Quaternion.IDENTITY

func _physics_process(delta: float):	
	# Check if we have authority over the parent entity
	if has_authority():
		apply_input(delta)

func apply_input(delta: float):
	if character_body == null:
		return
	
	# Interpolate motion for smoothness
	# motion was Vector2 previously, but now we work with Vector3 world direction
	# Let's keep 'motion' as the smoothed version of 'move_vector'
	# Wait, original code had motion as Vector2. Let's adapt.
	
	# Move Vector is the target world velocity (normalized * intensity)
	# We interpret move_vector.length() as speed intensity
	
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
		airborne_time = MIN_AIRBORNE_TIME
		jump.emit()

	jumping = false

	if on_air:
		pass
	elif aiming:
		# Interpolate current rotation to view rotation
		var q_from = orientation.basis.get_rotation_quaternion()
		var q_to = view_quaternion
		orientation.basis = Basis(q_from.slerp(q_to, delta * ROTATION_INTERPOLATE_SPEED))

		if shooting and character_body.fire_cooldown.time_left == 0:
			var shoot_origin = character_body.shoot_from.global_transform.origin
			# Shoot direction is forward from orientation (since we aligned with view)
			# or we can use the explicit shoot_target if provided. 
			# Original code used shoot_target.
			var shoot_dir = (shoot_target - shoot_origin).normalized()
			
			# If no target, use forward
			if shoot_dir.length() < 0.001:
				shoot_dir = orientation.basis.z

			var bullet = preload("res://content/gobot/bullet/bullet.tscn").instantiate()
			get_parent().add_child(bullet, true)
			bullet.global_transform.origin = shoot_origin
			bullet.look_at(shoot_origin + shoot_dir, Vector3.UP)
			bullet.add_collision_exception_with(self)
			
			shoot.emit()

	else: # Not in air or aiming, idle/moving.
		# Rotate to face movement direction
		var target_dir = move_vector
		target_dir.y = 0 # Keep flat
		
		# Only rotate if we are moving
		if target_dir.length() > 0.001:
			var q_from = orientation.basis.get_rotation_quaternion()
			var q_to = Transform3D().looking_at(target_dir, Vector3.UP).basis.get_rotation_quaternion()
			orientation.basis = Basis(q_from.slerp(q_to, delta * ROTATION_INTERPOLATE_SPEED))

	# Apply root motion to orientation.
	if use_root_motion:
		orientation *= root_motion
	else:
		var target_velocity = move_vector
		target_velocity.y = 0
		orientation.origin += target_velocity * move_speed * delta

	do_move(delta)
	orientation.origin = Vector3()
	orientation = orientation.orthonormalized()
	

func do_move(delta):
	var h_velocity = orientation.origin / delta
	character_body.velocity.x = h_velocity.x
	character_body.velocity.z = h_velocity.z
	character_body.velocity += gravity * delta
	#character_body.set_velocity(character_body.velocity)
	character_body.set_up_direction(Vector3.UP)
	character_body.move_and_slide()

func get_command_metadata() -> Dictionary:
	return {
		"JUMP": {
			"description": "Make the character jump."
		},
		"SET_SPEED": {
			"description": "Set the character's movement speed.",
			"arguments": {
				"value": {
					"type": "float",
					"description": "Speed in m/s (default: 5.0)"
				}
			}
		}
	}

# Command Methods
func cmd_jump():
	jumping = true
	return "Jumping"

func cmd_set_move_vector(x: float, y: float, z: float):
	move_vector = Vector3(x, y, z)
	input_motion = Vector2(x, z) 
	# Also update 'motion' vec2 for animation blend if needed?
	# Original code used 'motion' (Vector2) for some logic, but we replaced it with move_vector (Vector3) in apply_input.
	# We might need to map it back if other systems read 'motion'.
	motion = Vector2(x, z) # Approximation for 2D blend space
	return "Move vector set"

func cmd_set_view_quaternion(x: float, y: float, z: float, w: float):
	view_quaternion = Quaternion(x, y, z, w)
	return "View quaternion set"

func cmd_set_aiming(is_aiming: bool):
	aiming = is_aiming
	return "Aiming: %s" % is_aiming

func cmd_set_shooting(is_shooting: bool):
	shooting = is_shooting
	return "Shooting: %s" % is_shooting

func cmd_set_shoot_target(x: float, y: float, z: float):
	shoot_target = Vector3(x, y, z)
	return "Shoot target set"

func cmd_set_speed(value: float = 5.0):
	move_speed = value
	return "Speed set to %.1f" % move_speed
