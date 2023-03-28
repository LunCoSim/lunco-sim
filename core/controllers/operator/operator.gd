#This code is based on Gogot kinematic_character example
class_name lnOperator
extends CharacterBody3D

@export var MAX_SPEED = 100
@export var JUMP_SPEED = 2
@export var ACCELERATION = 50
@export var DECELERATION = 50

@onready var start_position = position

#var velocity := Vector3.ZERO
var dir := Vector3.ZERO
var orientation := Basis.IDENTITY

# Commands
# reset_position
# move(direction)
	
		
func _physics_process(delta):

	var target_dir = orientation * dir * MAX_SPEED
	var acceleration
	
	if dir.dot(velocity) > 0:
		acceleration = ACCELERATION
	else:
		acceleration = DECELERATION

	velocity = velocity.lerp(target_dir, acceleration * delta)

	move_and_slide()

	
#-----------

func reset_position():
	position = start_position

func move(direction):
	dir = direction.normalized()
	
func orient(_orientation):
	orientation = _orientation
