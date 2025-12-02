#This code is based on this game: https://godotforums.org/discussion/18480/godot-3d-vector-physics-cheat-sheet
@icon("res://controllers/spacecraft/rocket.svg")
class_name LCSpacecraftController
extends LCController

@export_category("Rocket Specific parameters")
# Constants handled by LCSpacecraft now
# @export var THRUST = 50
# @export var THRUST_TURN = 200
# @export var THRUST_ROLL = 50

@onready var parent: 
	get:
		return self.get_parent()


signal thrusted(enabled)

func _ready():
	print("LCSpacecraftController: Ready. Parent: ", parent.name if parent else "NULL")
	if parent:
		print("LCSpacecraftController: Parent methods: ", parent.get_method_list().map(func(m): return m.name).filter(func(n): return "control" in n))

# Commands
# thrust
# change orienation(x, y, z)

# Internal state
var thrust := 0.0
var torque := Vector3.ZERO

#-----------------------

# Processing physics for Spacecraft controller
# func _physics_process(_delta):
# 	# Delegated to LCSpacecraft
# 	pass
	
# ------------
# Commands that changes internal state
func throttle(_thrust: bool):
	if _thrust:
		thrust = 1.0
		print("SpacecraftController: Throttle activated")
	else:
		thrust = 0.0
		
	thrusted.emit(_thrust)
	_update_parent_control()
		
func change_orientation(_torque: Vector3):
	torque = _torque
	_update_parent_control()

func _update_parent_control():
	if parent and parent.has_method("set_control_inputs"):
		parent.set_control_inputs(thrust, torque)
	elif parent and parent.has_method("_on_spacecraft_controller_thrusted"):
		# Fallback for older spacecraft
		parent._on_spacecraft_controller_thrusted(thrust > 0.5)
