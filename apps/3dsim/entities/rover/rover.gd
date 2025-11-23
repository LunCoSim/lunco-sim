@tool
class_name LCControllableRover
extends LCVehicle

var _owner_id: int = 0
var controller: Node
var input_adapter: Node

func _ready():
	super._ready()
	
	# Skip game logic in editor
	if Engine.is_editor_hint():
		return
		
	# Find and cache the controller and input adapter
	controller = get_node_or_null("RoverController")
	input_adapter = get_node_or_null("RoverInputAdapter")
	
	# Connect the input adapter to the controller if both exist
	if input_adapter and controller:
		if input_adapter.has_method("_set_target"):
			input_adapter.call("_set_target", controller)
		else:
			input_adapter.target = controller
	
	# Set initial position
	teleport_to_safe_position()

func take_control(id: int) -> bool:
	if _owner_id != 0:
		return false
	
	_owner_id = id
	set_multiplayer_authority(id)
	
	# Reset controls if controller exists
	if controller:
		if controller.has_method("take_control"):
			controller.take_control()
		
		if controller.has_method("set_motor"):
			controller.set_motor(0.0)
		if controller.has_method("set_steering"):
			controller.set_steering(0.0)
		if controller.has_method("set_brake"):
			controller.set_brake(0.0)
	
	# Connect input adapter
	if input_adapter and controller:
		input_adapter.target = controller
	
	return true

func release_control(id: int) -> bool:
	if _owner_id != id and id != 0:
		return false
	
	_owner_id = 0
	set_multiplayer_authority(1)
	
	# Reset the input adapter
	if input_adapter:
		input_adapter.target = null
	
	return true

func get_owner_id() -> int:
	return _owner_id

# Compatibility with spacecraft control system
func _on_spacecraft_controller_thrusted(_enabled: bool):
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

# Add a function to teleport the rover to a safe position
func teleport_to_safe_position():
	global_position = Vector3(0, 1.0, 0)  # Slightly above ground level
	global_rotation = Vector3.ZERO       # Reset rotation
	linear_velocity = Vector3.ZERO       # Reset velocity
	angular_velocity = Vector3.ZERO      # Reset angular velocity
	
	# Apply a small downward force to ensure contact with ground
	apply_central_impulse(Vector3(0, -10, 0))

		
 
