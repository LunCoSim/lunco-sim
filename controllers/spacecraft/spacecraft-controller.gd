#This code is based on this game: https://godotforums.org/discussion/18480/godot-3d-vector-physics-cheat-sheet
@icon("res://controllers/spacecraft/rocket.svg")
class_name LCSpacecraftController
extends LCSpaceSystem

@export_category("Rocket Specific parameters")
@export var THRUST = 50
@export var THRUST_TURN = 200
@export var THRUST_ROLL = 50

@onready var parent: RigidBody3D: 
	get:
		return self.get_parent()


signal thrusted(enabled)

const Z_FRONT = 1 #in this game the front side is towards negative Z

# Commands
# thrust
# change orienation(x, y, z)

# Internal state
var thrust := 0.0
var torque := Vector3.ZERO

#-----------------------

# Processing physics for Spacecraft controller
func _physics_process(delta):
	#if Target.name == str(multiplayer.get_unique_id()): TBD Find a better way to handle multiplayer authority
	if is_multiplayer_authority():
		if parent:
			parent.apply_central_force(parent.transform.basis.z * Z_FRONT * thrust)

			parent.apply_torque(parent.global_transform.basis.x * torque.x * THRUST_TURN * Z_FRONT)
			parent.apply_torque(parent.global_transform.basis.y * torque.y * THRUST_TURN * Z_FRONT)
			parent.apply_torque(parent.global_transform.basis.z * torque.z * THRUST_ROLL * Z_FRONT)

func _input(event):
	if Input.is_action_just_pressed("throttle"):
		throttle(true)
	elif Input.is_action_just_released("throttle"):
		throttle(false)

	var torque := Vector3(
		Input.get_action_strength("pitch_up") - Input.get_action_strength("pitch_down"),
		Input.get_action_strength("yaw_right") - Input.get_action_strength("yaw_left"),
		Input.get_action_strength("roll_cw") - Input.get_action_strength("roll_ccw")
	)

	change_orientation(torque)
	
# ------------
# Commands that changes internal state
func throttle(_thrust: bool):
	emit_signal("thrusted", _thrust)
	
	if _thrust:
		thrust = THRUST
	else:
		thrust = 0
		
func change_orientation(_torque: Vector3):
	torque = _torque
