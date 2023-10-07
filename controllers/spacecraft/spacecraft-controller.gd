#This code is based on this game: https://godotforums.org/discussion/18480/godot-3d-vector-physics-cheat-sheet
@icon("res://controllers/spacecraft/rocket.svg")
class_name LCSpacecraftController
extends LCSpaceSystem

@export var THRUST = 50
@export var THRUST_TURN = 200
@export var THRUST_ROLL = 50

@onready var Target: RigidBody3D = get_parent()

signal thrusted(enabled)

const Z_FRONT = 1 #in this game the front side is towards negative Z



# Commands
# thrust
# change orienation(x, y, z)

var thrust := 0.0
var torque := Vector3.ZERO

#-----------------------

		
func _physics_process(delta):
	#if Target.name == str(multiplayer.get_unique_id()): TBD Find a better way to handle multiplayer authority
	if is_multiplayer_authority():
		if Target:
			Target.apply_central_force(Target.transform.basis.z * Z_FRONT * thrust)

			Target.apply_torque(Target.global_transform.basis.x * torque.x * THRUST_TURN * Z_FRONT)
			Target.apply_torque(Target.global_transform.basis.y * torque.y * THRUST_TURN * Z_FRONT)
			Target.apply_torque(Target.global_transform.basis.z * torque.z * THRUST_ROLL * Z_FRONT)

	
# ------------

func throttle(_thrust: bool):
	emit_signal("thrusted", _thrust)
	
	if _thrust:
		thrust = THRUST
	else:
		thrust = 0
		
func change_orientation(orien: Vector3):
	torque = orien
