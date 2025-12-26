extends Node

# State
# State
enum BuilderState {
	IDLE,
	SELECTING_PART,
	PLACING_PART,
	VIEWING
}

enum SymmetryMode {
	NONE,
	X,      # Left/Right
	Z,      # Front/Back
	QUAD    # X + Z
}

var current_state = BuilderState.IDLE
var symmetry_mode = SymmetryMode.NONE
var selected_part_id: String = ""
var ghost_instance: Node3D = null
var ghost_mirrors: Array[Node3D] = [] # Stores symmetry ghosts


# Configuration
# Map part IDs to resource paths
var part_registry = {
	"chassis_box": "res://core/components/structure/chassis_box.tscn",
	"wheel_basic": "res://core/components/propulsion/wheel_basic.tscn",
	"battery_effector": "res://core/components/power/battery_effector.tscn",
	"solar_panel_effector": "res://core/components/power/solar_panel_effector.tscn",
	"resource_tank_effector": "res://core/components/power/resource_tank_effector.tscn",
	"thruster_effector": "res://core/components/propulsion/thruster_effector.tscn",
	"lidar_effector": "res://core/components/sensors/lidar_effector.tscn",
	"camera_effector": "res://core/components/sensors/camera_effector.tscn",
	"imu_effector": "res://core/components/sensors/imu_effector.tscn",
	"gps_effector": "res://core/components/sensors/gps_effector.tscn"
}

var categories = {
	"Structure": ["chassis_box"],
	"Propulsion": ["wheel_basic", "thruster_effector"],
	"Power": ["solar_panel_effector", "battery_effector", "resource_tank_effector"],
	"Sensors": ["lidar_effector", "camera_effector", "imu_effector", "gps_effector"]
}

signal part_removed(part)

func remove_part(part: Node):
	if part:
		var parent = part.get_parent()
		part.queue_free()
		if parent and (parent is LCVehicle):
			# Defer refresh
			parent.call_deferred("refresh_effectors")
		part_removed.emit(part)


func _ready():
	pass

func start_building():
	current_state = BuilderState.SELECTING_PART
	# Show UI

func stop_building():
	current_state = BuilderState.IDLE
	if ghost_instance:
		ghost_instance.queue_free()
		ghost_instance = null

func select_part(part_id: String):
	if selected_part_id == part_id and current_state == BuilderState.PLACING_PART:
		deselect_part()
		return

	if part_registry.has(part_id):
		selected_part_id = part_id
		current_state = BuilderState.PLACING_PART
		create_ghost(part_id)

signal part_selected(part_id)
signal part_deselected
signal entity_selected(entity)

func deselect_part():
	print("BuilderManager: Deselecting part")
	selected_part_id = ""
	current_state = BuilderState.SELECTING_PART
	if ghost_instance:
		ghost_instance.queue_free()
		ghost_instance = null
	part_deselected.emit()

func try_select_entity():
	var camera = get_viewport().get_camera_3d()
	if not camera: return
	
	var mouse_pos = get_viewport().get_mouse_position()
	var from = camera.project_ray_origin(mouse_pos)
	var to = from + camera.project_ray_normal(mouse_pos) * 1000.0
	
	var space_state = camera.get_world_3d().direct_space_state
	var query = PhysicsRayQueryParameters3D.create(from, to)
	query.collision_mask = 1 # Adjust mask if needed
	
	var result = space_state.intersect_ray(query)
	
	if result and result.collider:
		var target = result.collider
		# Walk up to find LCConstructible or LCSpacecraft
		while target:
			if target is LCConstructible or target is LCVehicle or target is LCSpacecraft or target.has_method("set_control_inputs"):
				break
			
			# Check by script path (robust against class_name issues)
			var script = target.get_script()
			if script and (script.resource_path.ends_with("spacecraft.gd") or script.resource_path.ends_with("starship.gd")):
				break
			# Fallback: Check by duck typing (safer if class_name fails)
			if target.has_method("register_component") and target.has_method("recalculate_physics"):
				print("BuilderManager: Found LCConstructible via duck typing")
				break
				
			target = target.get_parent()
		
		if target and (target is LCConstructible or target is LCVehicle or target is LCSpacecraft or target.has_method("register_component")):
			print("BuilderManager: Selected entity: ", target.name)
			entity_selected.emit(target)
		else:
			print("BuilderManager: Hit something, but not a constructible")
			print("Hit Object: ", result.collider.name)
			print("Hit Class: ", result.collider.get_class())
			if result.collider.get_script():
				print("Hit Script: ", result.collider.get_script().resource_path)
			
			entity_selected.emit(null) # Deselect
	else:
		print("BuilderManager: Selection raycast missed")
		# Optional: Deselect if clicked on empty space
		# entity_selected.emit(null)

func select_entity(entity):
	if entity and (entity is LCConstructible or entity is LCVehicle or entity is LCSpacecraft or entity.has_method("register_component") or entity.has_method("set_control_inputs")):
		print("BuilderManager: Programmatically selected entity: ", entity.name)
		entity_selected.emit(entity)
	else:
		print("BuilderManager: Programmatically deselected entity")
		entity_selected.emit(null)

func create_ghost(part_id: String):
	if ghost_instance:
		ghost_instance.queue_free()
	
	var scene = load(part_registry[part_id])
	if scene:
		ghost_instance = scene.instantiate()
		# Make visual only (disable colliders, scripts, etc if needed)
		# For now, just adding it to the tree might be enough if we handle input carefully
		add_child(ghost_instance)
		# TODO: Apply "Ghost" material

func _process(delta):
	if current_state == BuilderState.PLACING_PART and ghost_instance:
		update_ghost_position()

func _unhandled_input(event):
	if current_state == BuilderState.PLACING_PART and ghost_instance:
		if event.is_action_pressed("click") or event.is_action_pressed("ui_accept"):
			try_place_part()
			get_viewport().set_input_as_handled()
	elif event.is_action_pressed("click"):
		# Check if the click hit a UI display before processing
		if _is_click_on_ui_display():
			# Don't process clicks on UI displays
			return
		
		print("BuilderManager: Click detected in _unhandled_input")
		try_select_entity()

# Helper function to check if mouse is over a UI display
func _is_click_on_ui_display() -> bool:
	var camera = get_viewport().get_camera_3d()
	if not camera:
		return false
	
	var mouse_pos = get_viewport().get_mouse_position()
	var from = camera.project_ray_origin(mouse_pos)
	var to = from + camera.project_ray_normal(mouse_pos) * 1000.0
	
	var space_state = camera.get_world_3d().direct_space_state
	var query = PhysicsRayQueryParameters3D.create(from, to)
	query.collide_with_areas = true
	query.collision_mask = 2  # UI displays are on collision layer 2
	
	var result = space_state.intersect_ray(query)
	
	if result and result.collider:
		# Check if it's a UI display area
		var collider = result.collider
		if collider is Area3D and (collider.collision_layer & 2) != 0:
			print("BuilderManager: Click is on UI display, ignoring")
			return true
	
	return false

func update_ghost_position():
	# Raycast from camera
	var camera = get_viewport().get_camera_3d()
	if not camera: return
	
	var mouse_pos = get_viewport().get_mouse_position()
	var from = camera.project_ray_origin(mouse_pos)
	var to = from + camera.project_ray_normal(mouse_pos) * 100.0
	
	var space_state = camera.get_world_3d().direct_space_state
	var query = PhysicsRayQueryParameters3D.create(from, to)
	query.collision_mask = 1
	
	var result = space_state.intersect_ray(query)
	var target_parent = null
	
	if result:
		# Calculate AABB to align bottom to surface
		var aabb = _get_recursive_aabb(ghost_instance)
		var bottom_offset = -aabb.position.y
		
		# Align to surface normal
		var normal = result.normal
		var up = Vector3.UP
		
		# Position with offset along the normal
		ghost_instance.global_position = result.position + (normal * bottom_offset)
		
		# Basic alignment logic (can be improved)
		if abs(normal.dot(Vector3.UP)) > 0.9:
			# Floor/Ceiling
			ghost_instance.look_at(result.position + (normal * bottom_offset) + Vector3.FORWARD, normal)
		else:
			# Wall
			ghost_instance.look_at(result.position + (normal * bottom_offset) + normal, Vector3.UP)
			
		# Identify parent specifically for symmetry calculations (local space)
		var collider = result.collider
		while collider:
			if collider is LCConstructible or collider is LCVehicle:
				target_parent = collider
				break
			collider = collider.get_parent()
			
	else:
		# Float in front
		ghost_instance.global_position = from + camera.project_ray_normal(mouse_pos) * 5.0
		ghost_instance.rotation = Vector3.ZERO

	if not ghost_mirrors.is_empty():
		if target_parent:
			# Calculate local position relative to vehicle center
			var local_pos = target_parent.to_local(ghost_instance.global_position)
			var local_rot = target_parent.basis.inverse() * ghost_instance.global_basis
			
			# Apply symmetry
			for i in range(ghost_mirrors.size()):
				var mirror = ghost_mirrors[i]
				var mirror_pos = local_pos
				var mirror_rot = local_rot
				
				# Mirror Logic
				if symmetry_mode == SymmetryMode.X:
					mirror_pos.x *= -1
					# Mirror rotation across X plane
					# This is complex for arbitrary rotations, but for wheels/simple parts:
					mirror_rot.x.x *= -1 # Reflection
					# Correction? typically wheels need to flip 180 on Y or similar depending on mount? 
					# For now, simple position mirror is key. Rotation often needs context.
					
				elif symmetry_mode == SymmetryMode.Z:
					mirror_pos.z *= -1
					
				elif symmetry_mode == SymmetryMode.QUAD:
					if i == 0: # X Mirror
						mirror_pos.x *= -1
					elif i == 1: # Z Mirror
						mirror_pos.z *= -1
					elif i == 2: # X+Z Mirror
						mirror_pos.x *= -1
						mirror_pos.z *= -1
				
				mirror.global_position = target_parent.to_global(mirror_pos)
				# mirror.global_basis = target_parent.basis * mirror_rot # Rotation mirroring is hard, skip for MVP
				mirror.global_rotation = ghost_instance.global_rotation # Temp: match rotation
				mirror.visible = true # Ensure mirror is visible if parent found
		else:
			# If no parent, just hide mirrors or place them relative to world origin?
			# Hiding mirrors if not on a vehicle is safer
			for mirror in ghost_mirrors:
				mirror.visible = false
				mirror.global_position = ghost_instance.global_position # prevent glitches

func _get_recursive_aabb(node: Node3D) -> AABB:
	var aabb = AABB()
	var first = true
	
	for child in node.get_children():
		if child is VisualInstance3D:
			var child_aabb = child.get_aabb()
			# Transform to parent space
			child_aabb = child.transform * child_aabb
			
			if first:
				aabb = child_aabb
				first = false
			else:
				aabb = aabb.merge(child_aabb)
		
		# Recursively check children
		# Note: Getting deep recursion aligned might be tricky with transforms, 
		# for now assuming flat hierarchy for components or using VisualInstance3D AABB which is usually local.
		
	if first: # No visual children found, return default
		return AABB(Vector3(-0.5, -0.5, -0.5), Vector3(1, 1, 1))
		
	return aabb

func try_place_part():
	if selected_part_id == "chassis_box":
		# Constructible spawning (no symmetry for root)
		_spawn_constructible(ghost_instance.global_position, ghost_instance.global_rotation)
	else:
		# Attach component(s)
		var targets = []
		# Raycast again to be sure (or cache from update loop)
		var camera = get_viewport().get_camera_3d()
		if not camera: return
		var mouse_pos = get_viewport().get_mouse_position()
		var from = camera.project_ray_origin(mouse_pos)
		var to = from + camera.project_ray_normal(mouse_pos) * 100.0
		var space_state = camera.get_world_3d().direct_space_state
		var query = PhysicsRayQueryParameters3D.create(from, to)
		query.collision_mask = 1
		var result = space_state.intersect_ray(query)
		
		if result and result.collider:
			var target = result.collider
			while target and not (target is LCConstructible or target is LCVehicle):
				target = target.get_parent()
			
			if target and (target is LCConstructible or target is LCVehicle):
				# Place Primary
				_request_attach(target, selected_part_id, ghost_instance.global_position, ghost_instance.global_rotation)
				
				# Place Mirrors
				for mirror in ghost_mirrors:
					if mirror.visible:
						_request_attach(target, selected_part_id, mirror.global_position, mirror.global_rotation)

func _spawn_constructible(pos: Vector3, rot: Vector3):
	if multiplayer.has_multiplayer_peer() and multiplayer.get_peers().size() > 0:
		request_spawn_constructible.rpc_id(1, selected_part_id, pos, rot)
	else:
		request_spawn_constructible(selected_part_id, pos, rot)

func _request_attach(parent: Node, type: String, pos: Vector3, rot: Vector3):
	# Calculate relative transform for precise placement
	# But RPC currently only accepts generic Attach. 
	# We need to UPDATE existing RPC or just use what we have (center of parent??)
	# The current RPC `request_attach_component` does NOT take position!
	# We must update it to support position offsets.
	
	if multiplayer.has_multiplayer_peer() and multiplayer.get_peers().size() > 0:
		request_attach_component.rpc_id(1, parent.get_path(), type, pos, rot)
	else:
		request_attach_component(str(parent.get_path()), type, pos, rot)

# Server-side RPCs
@rpc("any_peer", "call_remote", "reliable")
func request_spawn_constructible(type: String, pos: Vector3, rot: Vector3):
	print("BuilderManager: request_spawn_constructible called for ", type)
	
	# In single-player, we are the "server"
	# In multiplayer, only the server should execute this
	if multiplayer.has_multiplayer_peer() and not multiplayer.is_server():
		print("BuilderManager: Not server, ignoring spawn request")
		return
	
	# Spawn new Constructible
	var constructible_scene = load("res://core/base/constructible.tscn")
	var constructible = constructible_scene.instantiate()
	constructible.name = "Rover_" + str(randi()) # Unique name
	
	# Find the Spawner node in the main scene
	var spawner = get_tree().current_scene.find_child("Spawner", true, false)
	if spawner:
		spawner.add_child(constructible)
		print("BuilderManager: Added constructible to Spawner")
	else:
		push_warning("BuilderManager: Spawner node not found, adding to root")
		get_tree().root.add_child(constructible)
		
	constructible.global_position = pos
	constructible.global_rotation = rot
	
	# Add the component
	var comp_scene = load(part_registry[type])
	var comp = comp_scene.instantiate()
	constructible.add_child(comp)
	
	if comp is LCComponent:
		constructible.register_component(comp)
	else:
		print("BuilderManager: Spawned part is not an LCComponent: ", comp.name)
	
	# Notify the simulation about the new entity
	var simulation = get_tree().current_scene
	if simulation and simulation.has_method("_on_multiplayer_spawner_spawned"):
		simulation._on_multiplayer_spawner_spawned(constructible)
		print("BuilderManager: Notified simulation of new entity")
	else:
		push_warning("BuilderManager: Could not find simulation to notify")
	
	# Ensure networking spawns it on clients (needs MultiplayerSpawner setup in scene)


@rpc("any_peer", "call_remote", "reliable")
func request_attach_component(parent_path: String, type: String, pos: Vector3, rot: Vector3):
	print("BuilderManager: request_attach_component called for ", type, " on ", parent_path)
	
	if multiplayer.has_multiplayer_peer() and not multiplayer.is_server():
		return
	
	var parent = get_node(parent_path)
	if parent and (parent is LCConstructible or parent is LCVehicle):
		var comp_scene = load(part_registry[type])
		if comp_scene:
			var comp = comp_scene.instantiate()
			parent.add_child(comp)
			
			# Apply transform relative to parent
			# We received GLOBAL pos from client, need to convert to LOCAL
			comp.global_position = pos
			comp.global_rotation = rot
			
			if parent is LCConstructible and comp is LCComponent:
				parent.register_component(comp)
			elif parent is LCVehicle:
				if parent.has_method("refresh_effectors"):
					parent.refresh_effectors()
					
			# Notify change signal
			if not is_instance_valid(ghost_instance): # Avoid double signal if we are the one building
				# This part is tricky, we just emit 'entity_selected' to trigger UI refresh
				entity_selected.emit(parent)
				
		else:
			push_error("BuilderManager: Failed to load: " + type)
	else:
		push_error("BuilderManager: Parent not found: " + parent_path)
