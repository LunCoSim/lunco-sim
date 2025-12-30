#This code is based on Gogot kinematic_character example
@icon("res://controllers/operator/operator.svg")
class_name LCOperatorController
extends LCController

@onready var Target: CharacterBody3D = get_parent()

@export var MAX_SPEED = 100
@export var JUMP_SPEED = 2
@export var ACCELERATION = 50
@export var DECELERATION = 50

@onready var start_position = position

# Internal state
var move_direction := Vector3.ZERO

func _ready():
	# Add command executor
	var executor = LCCommandExecutor.new()
	executor.name = "CommandExecutor"
	add_child(executor)

func _physics_process(delta):
	if has_authority():
		if Target:
			# move_direction is already in world space (normalized)
			var target_velocity = move_direction * MAX_SPEED
			var acceleration
			
			if move_direction.dot(Target.velocity) > 0:
				acceleration = ACCELERATION
			else:
				acceleration = DECELERATION

			Target.velocity = Target.velocity.lerp(target_velocity, acceleration * delta)
			Target.move_and_slide()

# Command Methods
func cmd_move(x: float, y: float, z: float):
	move_direction = Vector3(x, y, z)
	# Check for invalid length if needed, but input adapter should handle normalization
	if move_direction.length() > 1.0:
		move_direction = move_direction.normalized()
	return "Move direction set"

func cmd_reset_position():
	position = start_position
	# Also reset velocity
	if Target:
		Target.velocity = Vector3.ZERO
	return "Position reset"

# Legacy compatibility (can be removed if all adapters updated)
func reset_position():
	cmd_reset_position()

func move(direction):
	# Implicitly assumes local direction if called directly, but we are moving to commands
	# Let's map it to command for now
	cmd_move(direction.x, direction.y, direction.z)
	
func orient(_orientation):
	# Deprecated: Orientation should be handled by caller converting input to world space
	pass
