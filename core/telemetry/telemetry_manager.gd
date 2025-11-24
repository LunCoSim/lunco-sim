extends Node

# TelemetryManager - Observes entities and tracks events automatically
# Entities have no knowledge of telemetry

# Historical data storage (circular buffer per entity)
const MAX_HISTORY_SAMPLES = 10000  # Store last 10,000 property snapshots per entity (~16 minutes at 10Hz)
const COLLECTION_RATE_HZ = 10.0

var entity_trackers: Dictionary = {}  # entity_id -> TelemetryEntityTracker
var entity_history: Dictionary = {}  # entity_id -> Array of property snapshots
var all_events: Array = []  # Global event log
var collection_timer: Timer
var discovery_timer: Timer

const MAX_GLOBAL_EVENTS = 50000  # Store last 50,000 global events

func _ready():
	# Create collection timer
	collection_timer = Timer.new()
	collection_timer.wait_time = 1.0 / COLLECTION_RATE_HZ
	collection_timer.timeout.connect(_collect_telemetry)
	add_child(collection_timer)
	collection_timer.start()
	
	# Create entity discovery timer (slower rate)
	discovery_timer = Timer.new()
	discovery_timer.wait_time = 1.0  # Check for new entities every second
	discovery_timer.timeout.connect(_discover_entities)
	add_child(discovery_timer)
	discovery_timer.start()
	
	# Connect to user manager for user events
	if Users:
		Users.user_connected.connect(_on_user_connected)
		Users.user_disconnected.connect(_on_user_disconnected)
	
	# Connect to simulation signals for entity lifecycle
	call_deferred("_connect_to_simulation")
	
	print("TelemetryManager initialized - auto-discovery enabled at %d Hz" % COLLECTION_RATE_HZ)
	
	# Initial discovery
	call_deferred("_discover_entities")

func _connect_to_simulation():
	# Find the Simulation node
	var simulation = get_tree().root.find_child("Simulation", true, false)
	if simulation:
		if simulation.has_signal("entity_spawned"):
			simulation.entity_spawned.connect(_on_entity_spawned)
			print("TelemetryManager connected to Simulation.entity_spawned")
		if simulation.has_signal("entities_updated"):
			simulation.entities_updated.connect(_on_entities_updated)
			print("TelemetryManager connected to Simulation.entities_updated")
	else:
		print("TelemetryManager: Simulation node not found, using fallback discovery")

func _on_entity_spawned(entity: Node):
	# Immediately track newly spawned entity
	if _should_track_entity(entity):
		var entity_id = str(entity.get_instance_id())
		if not entity_trackers.has(entity_id):
			var tracker = TelemetryEntityTracker.new(entity)
			entity_trackers[entity_id] = tracker
			_track_global_event("entity.spawned", {
				"entity_id": entity_id,
				"entity_name": entity.name,
				"entity_type": tracker.entity_type
			})
			print("TelemetryManager: Tracking new entity: ", entity.name, " (", tracker.entity_type, ")")

func _on_entities_updated(entities: Array):
	# Update tracking when entities list changes
	print("TelemetryManager: Entities updated, count: ", entities.size())
	_discover_entities()

func _discover_entities():
	# Method 1: Check Simulation.entities array if available
	# Find by class name instead of node name to support "Simulation", "Simulation2", etc.
	var simulation = null
	for child in get_tree().root.get_children():
		if child is LCSimulation:
			simulation = child
			break
	
	if simulation:
		if "entities" in simulation:
			for entity in simulation.entities:
				if is_instance_valid(entity) and _should_track_entity(entity):
					var entity_id = str(entity.get_instance_id())
					if not entity_trackers.has(entity_id):
						var tracker = TelemetryEntityTracker.new(entity)
						entity_trackers[entity_id] = tracker
						_track_global_event("entity.discovered", {
							"entity_id": entity_id,
							"entity_name": entity.name,
							"entity_type": tracker.entity_type
						})
						print("TelemetryManager: Discovered entity from Simulation: ", entity.name)
		
			
		# Method 2: Fallback - scan scene tree for spawn_node children
		if "spawn_node" in simulation:
			var spawn_node = simulation.get("spawn_node")
			if spawn_node:
				for child in spawn_node.get_children():
					if _should_track_entity(child):
						var entity_id = str(child.get_instance_id())
						if not entity_trackers.has(entity_id):
							var tracker = TelemetryEntityTracker.new(child)
							entity_trackers[entity_id] = tracker
							_track_global_event("entity.discovered", {
								"entity_id": entity_id,
								"entity_name": child.name,
								"entity_type": tracker.entity_type
							})
							print("TelemetryManager: Discovered entity from spawn_node: ", child.name)

	
	# Clean up trackers for destroyed entities
	var to_remove = []
	for entity_id in entity_trackers.keys():
		var tracker = entity_trackers[entity_id]
		if not is_instance_valid(tracker.entity):
			tracker.cleanup()
			to_remove.append(entity_id)
			print("TelemetryManager: Entity destroyed: ", entity_id)
	
	for entity_id in to_remove:
		entity_trackers.erase(entity_id)

func _should_track_entity(node: Node) -> bool:
	# Track RigidBody3D, CharacterBody3D, and VehicleBody3D nodes
	if node is RigidBody3D or node is CharacterBody3D or node is VehicleBody3D:
		# Exclude certain nodes (e.g., projectiles, small objects)
		if node.name.begins_with("@"):  # Skip internal nodes
			return false
		return true
	return false

func _collect_telemetry():
	# Update properties for all tracked entities and store history
	for entity_id in entity_trackers.keys():
		var tracker = entity_trackers[entity_id]
		var props = tracker.update_properties()
		
		# Store property snapshot in history
		if not entity_history.has(entity_id):
			entity_history[entity_id] = []
		
		entity_history[entity_id].append(props.duplicate())
		
		# Trim history if too large
		if entity_history[entity_id].size() > MAX_HISTORY_SAMPLES:
			entity_history[entity_id].pop_front()

func _on_user_connected(user_id: int, username: String, _wallet: String):
	_track_global_event("user.connected", {
		"user_id": user_id,
		"username": username
	})

func _on_user_disconnected(user_id: int):
	_track_global_event("user.disconnected", {
		"user_id": user_id
	})

func _track_global_event(event_type: String, data: Dictionary):
	var event = {
		"timestamp": int(Time.get_unix_time_from_system() * 1000),
		"event_type": event_type,
		"data": data
	}
	
	all_events.append(event)
	
	# Trim events if too many
	if all_events.size() > MAX_GLOBAL_EVENTS:
		all_events.pop_front()

# API Methods for TelemetryRouter

func get_entities() -> Array:
	var result = []
	for tracker in entity_trackers.values():
		if is_instance_valid(tracker.entity):
			result.append({
				"entity_id": tracker.entity_id,
				"entity_name": tracker.entity_name,
				"entity_type": tracker.entity_type,
				"created_at": tracker.created_at
			})
	return result

func get_latest_telemetry(entity_id: String) -> Dictionary:
	if entity_trackers.has(entity_id):
		return entity_trackers[entity_id].last_properties
	return {}

func get_history(entity_id: String, start_time: int = 0, end_time: int = 0) -> Array:
	if not entity_history.has(entity_id):
		return []
	
	var history = entity_history[entity_id]
	
	# If no time range specified, return all history
	if start_time == 0 and end_time == 0:
		return history.duplicate()
	
	# Filter by time range
	var filtered = []
	for sample in history:
		var timestamp = sample.get("timestamp", 0)
		if (start_time == 0 or timestamp >= start_time) and (end_time == 0 or timestamp <= end_time):
			filtered.append(sample)
	
	return filtered

func get_entity_events(entity_id: String, start_time: int = 0, end_time: int = 0) -> Array:
	if entity_trackers.has(entity_id):
		return entity_trackers[entity_id].get_events(start_time, end_time)
	return []

func get_global_events(start_time: int = 0, end_time: int = 0) -> Array:
	if start_time == 0 and end_time == 0:
		return all_events.duplicate()
	
	var filtered = []
	for event in all_events:
		var timestamp = event.get("timestamp", 0)
		if (start_time == 0 or timestamp >= start_time) and (end_time == 0 or timestamp <= end_time):
			filtered.append(event)
	
	return filtered

func get_openmct_dictionary() -> Dictionary:
	var measurements = []
	
	for tracker in entity_trackers.values():
		if not is_instance_valid(tracker.entity):
			continue
			
		var entity_name = tracker.entity_name
		var entity_type = tracker.entity_type
		
		# Create measurement object for this entity
		var measurement = {
			"key": tracker.entity_id,
			"name": entity_name + " (" + entity_type + ")",
			"values": [
				{"key": "timestamp", "name": "Timestamp", "format": "utc", "hints": {"domain": 1}},
				{"key": "position.x", "name": "Position X", "unit": "m", "format": "float", "hints": {"range": 1}},
				{"key": "position.y", "name": "Position Y", "unit": "m", "format": "float", "hints": {"range": 1}},
				{"key": "position.z", "name": "Position Z", "unit": "m", "format": "float", "hints": {"range": 1}},
				{"key": "velocity.x", "name": "Velocity X", "unit": "m/s", "format": "float", "hints": {"range": 1}},
				{"key": "velocity.y", "name": "Velocity Y", "unit": "m/s", "format": "float", "hints": {"range": 1}},
				{"key": "velocity.z", "name": "Velocity Z", "unit": "m/s", "format": "float", "hints": {"range": 1}},
				{"key": "controller_id", "name": "Controller ID", "format": "integer", "hints": {"range": 1}}
			]
		}
		
		measurements.append(measurement)
	
	return {"measurements": measurements}
