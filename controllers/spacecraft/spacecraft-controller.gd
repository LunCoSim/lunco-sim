#This code is based on this game: https://godotforums.org/discussion/18480/godot-3d-vector-physics-cheat-sheet
@icon("res://controllers/spacecraft/rocket.svg")
class_name LCSpacecraftController
extends LCController

@export_category("Rocket Specific parameters")
@export var THRUST = 50
@export var THRUST_TURN = 200
@export var THRUST_ROLL = 50

@onready var parent: RigidBody3D: 
	get:
		return self.get_parent()


signal thrusted(enabled)

# Commands
# thrust
# change orienation(x, y, z)

# Internal state
var thrust := 0.0
var torque := Vector3.ZERO

#-----------------------

# Processing physics for Spacecraft controller
func _physics_process(_delta):
	#if Target.name == str(multiplayer.get_unique_id()): TBD Find a better way to handle multiplayer authority
	if is_multiplayer_authority():
		if parent:
			parent.apply_central_force(parent.transform.basis.z * thrust)

			parent.apply_torque(parent.global_transform.basis.x * torque.x * THRUST_TURN)
			parent.apply_torque(parent.global_transform.basis.y * torque.y * THRUST_TURN)
			parent.apply_torque(parent.global_transform.basis.z * torque.z * THRUST_ROLL)


	
# ------------
# Commands that changes internal state
func throttle(_thrust: bool):
	if _thrust:
		thrust = THRUST
		parent._on_spacecraft_controller_thrusted(true)     
	else:
		thrust = 0
		parent._on_spacecraft_controller_thrusted(false)
		
func change_orientation(_torque: Vector3):
	torque = _torque
