class_name LCControllableRover
extends VehicleBody3D

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
		# Connect the input adapter to the controller
		if input_adapter.has_method("_set_target") and controller:
			input_adapter.call("_set_target", controller)
		elif controller:
			input_adapter.target = controller
			print("Rover: Set input adapter target to controller")

func take_control(id: int) -> bool:
	print("Rover: take_control called with id=", id)
	if _owner_id != 0:
		print("Rover: Already controlled by ", _owner_id)
		return false
	
	_owner_id = id
	set_multiplayer_authority(id)
	print("Rover: Control granted to player ", id)
	
	# Ensure the input adapter is connected
	if input_adapter and controller:
		input_adapter.target = controller
		print("Rover: Connected input adapter to controller on take_control")
	
	return true

func release_control(id: int) -> bool:
	print("Rover: release_control called with id=", id)
	if _owner_id != id and id != 0:
		print("Rover: Cannot release - controlled by ", _owner_id, " not ", id)
		return false
	
	_owner_id = 0
	set_multiplayer_authority(1)
	print("Rover: Control released by player ", id)
	
	# Reset the input adapter
	if input_adapter:
		input_adapter.target = null
	
	return true

func get_owner_id() -> int:
	return _owner_id

# Called every physics frame
func _physics_process(_delta):
	# Verify that the input adapter is properly connected to the controller
	if input_adapter and controller and is_multiplayer_authority():
		if input_adapter.target != controller:
			input_adapter.target = controller
			print("Rover: Reconnected input adapter to controller in physics_process")

# Compatibility with spacecraft control system
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
