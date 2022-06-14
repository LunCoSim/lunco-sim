#This code is based on this game: https://godotforums.org/discussion/18480/godot-3d-vector-physics-cheat-sheet
class_name Spacecraft
extends RigidBody

signal thrust(enabled)

const Z_FRONT = 1 #in this game the front side is towards negative Z

export var THRUST = 50
export var THRUST_TURN = 200
export var THRUST_ROLL = 50

# Commands
# thrust
# change orienation(x, y, z)

var thrust := 0.0
var torque := Vector3.ZERO

# damping: see linear and angular damping parameters

func _physics_process(delta):
	add_central_force(transform.basis.z * Z_FRONT * thrust)
	
	torque = Vector3(1*THRUST_TURN, 2*THRUST_TURN, 3*THRUST_ROLL)	
#	add_torque(torque)
	add_torque(global_transform.basis.x * torque.x * THRUST_TURN * Z_FRONT)
	add_torque(global_transform.basis.y * torque.y * THRUST_TURN * Z_FRONT)
	add_torque(global_transform.basis.z * torque.z * THRUST_ROLL * Z_FRONT)

# ------------

func throttle(_thrust: bool):
	emit_signal("thrust", _thrust)
	
	if _thrust:
		thrust = THRUST
	else:
		thrust = 0
		
func change_orientation(orien: Vector3):
	torque = orien
