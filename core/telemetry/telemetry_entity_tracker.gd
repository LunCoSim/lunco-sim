class_name TelemetryEntityTracker
extends RefCounted

# Tracks a single entity for telemetry purposes

var entity: Node
var entity_id: String
var entity_name: String
var entity_type: String
var created_at: int
var signal_connections: Array = []
var events: Array = []
var last_properties: Dictionary = {}
var _cached_controller: Node = null

var _property_map: Dictionary = {}
var _has_standard_methods: Dictionary = {
	"get_global_position": false,
	"get_global_rotation": false,
	"linear_velocity": false,
	"velocity": false,
	"get_owner_id": false
}

const MAX_EVENTS = 5000  # Keep last 5000 events per entity

func _init(tracked_entity: Node):
	entity = tracked_entity
	entity_id = str(entity.get_instance_id())
	entity_name = entity.name
	entity_type = _determine_entity_type(entity)
	created_at = int(Time.get_unix_time_from_system() * 1000)
	
	# Cache standard method existence
	_has_standard_methods.get_global_position = entity.has_method("get_global_position") or "global_position" in entity
	_has_standard_methods.get_global_rotation = entity.has_method("get_global_rotation") or "global_rotation" in entity
	_has_standard_methods.linear_velocity = "linear_velocity" in entity
	_has_standard_methods.velocity = "velocity" in entity
	_has_standard_methods.get_owner_id = entity.has_method("get_owner_id")
	
	# Parse Telemetry schema if present
	var telemetry_schema = entity.get("Telemetry")
	if telemetry_schema is Dictionary:
		for key in telemetry_schema:
			var prop_name = telemetry_schema[key]
			# Verify property exists (simple check)
			if prop_name in entity:
				_property_map[key] = prop_name
	
	# Connect to common signals
	_connect_signals()
	
	# Track creation event
	track_event("entity.spawned", {
		"position": _get_position(),
		"entity_type": entity_type
	})

func _determine_entity_type(node: Node) -> String:
	# Determine entity type based on class hierarchy
	if node is CharacterBody3D:
		return "character"
	elif node is RigidBody3D:
		# Check for more specific types
		if node.get_class() == "LCControllableRover" or "rover" in node.name.to_lower():
			return "rover"
		elif "spacecraft" in node.name.to_lower():
			return "spacecraft"
		else:
			return "rigidbody"
	else:
		return "unknown"

func _connect_signals():
	# Connect to control signals if they exist
	if entity.has_signal("control_granted"):
		entity.control_granted.connect(_on_control_granted)
		signal_connections.append("control_granted")
	
	if entity.has_signal("control_released"):
		entity.control_released.connect(_on_control_released)
		signal_connections.append("control_released")
	


func _on_control_granted(controller_id: int):
	track_event("control.granted", {"controller_id": controller_id})

func _on_control_released(controller_id: int):
	track_event("control.released", {"controller_id": controller_id})



func update_properties() -> Dictionary:
	if not is_instance_valid(entity):
		return {}
	
	var props = {
		"entity_id": entity_id,
		"entity_name": entity_name,
		"entity_type": entity_type,
		"timestamp": int(Time.get_unix_time_from_system() * 1000)  # Integer milliseconds
	}
	
	# Get position - use flat keys for OpenMCT compatibility
	if _has_standard_methods.get_global_position:
		var pos = entity.global_position
		# Always send position even if zero, so graphs don't break
		props["position.x"] = pos.x
		props["position.y"] = pos.y
		props["position.z"] = pos.z
	
	# Get rotation
	if _has_standard_methods.get_global_rotation:
		var rot = entity.global_rotation
		props["rotation.x"] = rot.x
		props["rotation.y"] = rot.y
		props["rotation.z"] = rot.z
	
	# Get velocity based on entity type
	if _has_standard_methods.linear_velocity:
		var vel = entity.linear_velocity
		props["velocity.x"] = vel.x
		props["velocity.y"] = vel.y
		props["velocity.z"] = vel.z
		var ang_vel = entity.angular_velocity
		props["angular_velocity.x"] = ang_vel.x
		props["angular_velocity.y"] = ang_vel.y
		props["angular_velocity.z"] = ang_vel.z
		props["mass"] = entity.mass
	elif _has_standard_methods.velocity:
		var vel = entity.velocity
		props["velocity.x"] = vel.x
		props["velocity.y"] = vel.y
		props["velocity.z"] = vel.z
		props["is_on_floor"] = entity.is_on_floor()
	
	# Get controller info if available
	if _has_standard_methods.get_owner_id:
		props["controller_id"] = entity.get_owner_id()
	
	# Get custom properties from controller child
	if not _cached_controller:
		_cached_controller = entity.get_node_or_null("RoverController")
		if not _cached_controller:
			_cached_controller = entity.get_node_or_null("SpacecraftController")
	
	if _cached_controller and is_instance_valid(_cached_controller):
		if _cached_controller.has_method("get_motor"):
			props["inputs.motor"] = _cached_controller.get_motor()
		if _cached_controller.has_method("get_steering"):
			props["inputs.steering"] = _cached_controller.get_steering()
		if _cached_controller.has_method("get_brake"):
			props["inputs.brake"] = _cached_controller.get_brake()
			
	# Aggregate telemetry from all child components (effectors, etc.)
	_collect_component_telemetry(entity, props)
			
	# Get mapped properties from Telemetry schema
	for key in _property_map:
		var prop_name = _property_map[key]
		var val = entity.get(prop_name)
		if val != null:
			props[key] = val
	
	last_properties = props
	return props

func _collect_component_telemetry(node: Node, props: Dictionary):
	for child in node.get_children():
		if child is LCComponent:
			var component_telemetry = child.get_telemetry()
			if component_telemetry is Dictionary:
				# print("DEBUG: Merging telemetry from ", child.name, ": ", component_telemetry.keys())
				for key in component_telemetry:
					# We allow components to set properties. If multiple components 
					# use the same key, the last one wins (shoud be avoided in design)
					props[key] = component_telemetry[key]
		
		# Recurse to find nested components if necessary
		if child.get_child_count() > 0:
			_collect_component_telemetry(child, props)

func _get_position() -> Vector3:
	if entity.has_method("get_global_position"):
		return entity.global_position
	return Vector3.ZERO

func track_event(event_type: String, data: Dictionary = {}):
	var event = {
		"timestamp": int(Time.get_unix_time_from_system() * 1000),
		"event_type": event_type,
		"entity_id": entity_id,
		"data": data
	}
	
	events.append(event)
	
	# Trim events if too many
	if events.size() > MAX_EVENTS:
		events.pop_front()

func get_events(start_time: int = 0, end_time: int = 0) -> Array:
	if start_time == 0 and end_time == 0:
		return events.duplicate()
	
	var filtered = []
	for event in events:
		var timestamp = event.get("timestamp", 0)
		if (start_time == 0 or timestamp >= start_time) and (end_time == 0 or timestamp <= end_time):
			filtered.append(event)
	
	return filtered

func cleanup():
	# Disconnect all signals
	if is_instance_valid(entity):
		if entity.has_signal("control_granted") and entity.control_granted.is_connected(_on_control_granted):
			entity.control_granted.disconnect(_on_control_granted)
		if entity.has_signal("control_released") and entity.control_released.is_connected(_on_control_released):
			entity.control_released.disconnect(_on_control_released)

	
	# Track destruction event
	track_event("entity.destroyed", {
		"lifetime_ms": int(Time.get_unix_time_from_system() * 1000) - created_at
	})
