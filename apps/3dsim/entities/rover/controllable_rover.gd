class_name LCControllableRover
extends RigidBody3D

signal control_granted
signal control_released

var _owner_id: int = 0
var input_adapter: Node

func _ready():
	# Set up networking and control
	set_multiplayer_authority(1)
	
	# Find and cache the input adapter
	input_adapter = get_node_or_null("RoverInputAdapter")
	if not input_adapter:
		push_warning("Rover: No input adapter found!")

func take_control(id: int) -> bool:
	if _owner_id != 0:
		return false
	
	_owner_id = id
	set_multiplayer_authority(id)
	print("Rover: Control granted to player ", id)
	control_granted.emit()
	return true

func release_control(id: int) -> bool:
	if _owner_id != id:
		return false
	
	_owner_id = 0
	set_multiplayer_authority(1)
	print("Rover: Control released by player ", id)
	control_released.emit()
	return true

func get_owner_id() -> int:
	return _owner_id

# Ensure we have the required signals for the control system
func _get_configuration_warnings() -> PackedStringArray:
	var warnings = PackedStringArray()
	if not has_signal("control_granted") or not has_signal("control_released"):
		warnings.append("Missing required control signals")
	
	if not get_node_or_null("RoverController"):
		warnings.append("Missing RoverController node")
	
	if not get_node_or_null("RoverInputAdapter"):
		warnings.append("Missing RoverInputAdapter node")
	
	return warnings 
