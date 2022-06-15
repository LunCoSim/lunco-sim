#This code is based on Gogot kinematic_character example
class_name lnOperator
extends KinematicBody

export var MAX_SPEED = 100
export var JUMP_SPEED = 2
export var ACCELERATION = 50
export var DECELERATION = 50

onready var gravity = 0
onready var start_position = translation

var velocity: Vector3
var dir = Vector3()

# Commands
# reset_position
# move(direction)
	
		
func _physics_process(delta):
	
	dir = dir.normalized()

	# Apply gravity.
	velocity.y += delta * gravity

	# Using only the horizontal velocity, interpolate towards the input.
	var hvel = velocity
	hvel.y = 0

	var target_dir = dir * MAX_SPEED
	var acceleration
	if dir.dot(hvel) > 0:
		acceleration = ACCELERATION
	else:
		acceleration = DECELERATION

	hvel = hvel.linear_interpolate(target_dir, acceleration * delta)

	# Assign hvel's values back to velocity, and then move.
	velocity.x = hvel.x
	velocity.z = hvel.z
	velocity.y = hvel.y
	velocity = move_and_slide(velocity, Vector3.UP)

	
#-----------

func reset_position():
	translation = start_position

func move(direction):
	dir = direction
