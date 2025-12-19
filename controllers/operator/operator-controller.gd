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

#var velocity := Vector3.ZERO
var dir := Vector3.ZERO
var orientation := Basis.IDENTITY

# Commands
# reset_position
# move(direction)

func _physics_process(delta):
	if has_authority():
		if Target:
			var target_dir = orientation * dir * MAX_SPEED
			var acceleration
			
			if dir.dot(Target.velocity) > 0:
				acceleration = ACCELERATION
			else:
				acceleration = DECELERATION

			Target.velocity = Target.velocity.lerp(target_dir, acceleration * delta)

			Target.move_and_slide()

#-----------

#Commands: 
# reset_position
# start_moving
# stop_moving

# Parameters
# moving_direction
# orientation

# Telemetry
# position
# velocity

func reset_position():
	position = start_position

func move(direction):
	dir = direction.normalized()
	
func orient(_orientation):
	orientation = _orientation
