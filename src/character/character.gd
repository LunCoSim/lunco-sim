class_name LcCharacter
extends CharacterBody3D


@export var SPEED := 5.0
@export var JUMP_VELOCITY := 4.5

# Get the gravity from the project settings to be synced with RigidBody nodes.
var gravity = ProjectSettings.get_setting("physics/3d/default_gravity")

#Commands:
#Jump
#@export_enum ()

enum Commands {
	JUMP,
	START_MOVING, #direction
	STOP_MOVING
}

enum State {
	IDLE,
	MOVING,
	ON_AIR	
}

var state = State.IDLE
var try_jumping = false
var moving_direction := Vector2.ZERO

func execute_command(command: Commands):
	return execute_command_args(command, null)

func execute_command_args(command: Commands, args):
	match command:
		Commands.JUMP:
			try_jumping = true
		Commands.START_MOVING:
			moving_direction = args
		Commands.STOP_MOVING:
			moving_direction = Vector2.ZERO
		_:
			print("Unknown commad")

func _physics_process(delta):
	# Add the gravity.
	if not is_on_floor():
		velocity.y -= gravity * delta

	# Handle Jump.
#	if Input.is_action_just_pressed("ui_accept") and is_on_floor():
	if try_jumping and is_on_floor():
		velocity.y = JUMP_VELOCITY
	try_jumping = false
	
	
	var direction = (transform.basis * Vector3(moving_direction.x, 0, moving_direction.y)).normalized()
	if direction:
		velocity.x = direction.x * SPEED
		velocity.z = direction.z * SPEED
	else:
		velocity.x = move_toward(velocity.x, 0, SPEED)
		velocity.z = move_toward(velocity.z, 0, SPEED)

	move_and_slide()
