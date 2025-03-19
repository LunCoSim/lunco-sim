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
	else:
		print("Rover: Found controller: ", controller.name)
	
	# Find and cache the input adapter
	input_adapter = get_node_or_null("RoverInputAdapter")
	if not input_adapter:
		push_warning("Rover: No input adapter found!")
	else:
		print("Rover: Found input adapter: ", input_adapter.name)
	
	# Connect control signals to controller
	if controller.has_method("take_control") and not control_granted.is_connected(controller.take_control):
		control_granted.connect(controller.take_control)
		print("Rover: Connected control_granted signal to controller")
	
	if controller.has_method("release_control") and not control_released.is_connected(controller.release_control):
		control_released.connect(controller.release_control)
		print("Rover: Connected control_released signal to controller")
	
	# Create a periodic status reporter
	var timer = Timer.new()
	timer.wait_time = 4.0
	timer.one_shot = false
	timer.autostart = true
	timer.connect("timeout", Callable(self, "_on_timer_timeout"))
	add_child(timer)

func _on_timer_timeout():
	print("Rover status: owner_id=", _owner_id, " authority=", get_multiplayer_authority())

func take_control(id: int) -> bool:
	print("Rover: take_control called with id=", id)
	if _owner_id != 0:
		print("Rover: Already controlled by ", _owner_id)
		return false
	
	_owner_id = id
	set_multiplayer_authority(id)
	print("Rover: Control granted to player ", id)
	
	# Fix: Make sure signal emission works properly
	control_granted.emit()
	
	# Double check that signals are properly connected
	if not control_granted.is_connected(controller.take_control):
		print("Rover: WARNING - control_granted signal not connected, connecting now")
		control_granted.connect(controller.take_control)
		# Call the method directly to ensure it takes effect
		controller.take_control()
	
	return true

func release_control(id: int) -> bool:
	print("Rover: release_control called with id=", id)
	if _owner_id != id and id != 0:
		print("Rover: Cannot release - controlled by ", _owner_id, " not ", id)
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
