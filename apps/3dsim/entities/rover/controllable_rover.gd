class_name LCControllableRover
extends RigidBody3D

signal control_granted
signal control_released

var _owner_id: int = 0
var controller: Node
var input_adapter: Node

func _ready():
	# Set up networking and control
	set_multiplayer_authority(1)
	
	# Find and cache the controller
	controller = get_node_or_null("RoverController")
	if not controller:
		push_warning("Rover: No controller found!")
	
	# Find and cache the input adapter
	input_adapter = get_node_or_null("RoverInputAdapter")
	if not input_adapter:
		push_warning("Rover: No input adapter found!")
	
	# Connect control signals to controller
	if controller.has_method("take_control") and not control_granted.is_connected(controller.take_control):
		control_granted.connect(controller.take_control)
	
	if controller.has_method("release_control") and not control_released.is_connected(controller.release_control):
		control_released.connect(controller.release_control)

func take_control(id: int) -> bool:
	if _owner_id != 0:
		return false
	
	_owner_id = id
	set_multiplayer_authority(id)
	print("Rover: Control granted to player ", id)
	control_granted.emit()
	return true

func release_control(id: int) -> bool:
	if _owner_id != id and id != 0:
		return false
	
	_owner_id = 0
	set_multiplayer_authority(1)
	print("Rover: Control released by player ", id)
	control_released.emit()
	return true

func get_owner_id() -> int:
	return _owner_id

# Control response for avatar control system - for compatibility with spacecraft
func _on_spacecraft_controller_thrusted(enabled: bool):
	# This is a placeholder for compatibility with the spacecraft control system
	pass

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
