extends Node

# State
enum BuilderState {
	IDLE,
	SELECTING_PART,
	PLACING_PART,
	VIEWING
}

var current_state = BuilderState.IDLE
var selected_part_id: String = ""
var ghost_instance: Node3D = null

# Configuration
# Map part IDs to resource paths
var part_registry = {
	"chassis_box": "res://core/components/structure/chassis_box.tscn",
	"wheel_basic": "res://core/components/propulsion/wheel_basic.tscn",
	"battery_basic": "res://core/components/power/battery_basic.tscn",
	"solar_panel_basic": "res://core/components/power/solar_panel_basic.tscn",
	"command_unit_basic": "res://core/components/avionics/command_unit_basic.tscn",
	"camera_basic": "res://core/components/payload/camera_basic.tscn"
}

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
	if part_registry.has(part_id):
		selected_part_id = part_id
		current_state = BuilderState.PLACING_PART
		create_ghost(part_id)

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
		
		if Input.is_action_just_pressed("ui_accept") or Input.is_mouse_button_pressed(MOUSE_BUTTON_LEFT):
			try_place_part()

func update_ghost_position():
	# Raycast from camera
	var camera = get_viewport().get_camera_3d()
	if not camera: return
	
	var mouse_pos = get_viewport().get_mouse_position()
	var from = camera.project_ray_origin(mouse_pos)
	var to = from + camera.project_ray_normal(mouse_pos) * 100.0
	
	var space_state = camera.get_world_3d().direct_space_state
	var query = PhysicsRayQueryParameters3D.create(from, to)
	# Mask for Constructibles/Components
	query.collision_mask = 1 # Adjust as needed
	
	var result = space_state.intersect_ray(query)
	
	if result:
		var collider = result.collider
		# Check if we hit an attachment node or a component
		# Logic to snap to nearest attachment node
		ghost_instance.global_position = result.position
		# TODO: Implement snapping logic
	else:
		# Float in front of camera
		ghost_instance.global_position = from + camera.project_ray_normal(mouse_pos) * 5.0

func try_place_part():
	# Determine parent and position
	# For MVP, if we are placing a Chassis (root), we spawn a new Constructible
	# If we are placing a component, we attach to existing
	
	if selected_part_id == "chassis_box":
		print("BuilderManager: Requesting spawn of chassis_box")
		
		# Check if we're in actual multiplayer mode (not just hosting locally)
		var peer = multiplayer.multiplayer_peer
		var is_server = multiplayer.is_server()
		var peer_count = multiplayer.get_peers().size()
		
		print("BuilderManager: Peer: ", peer)
		print("BuilderManager: Is server: ", is_server)
		print("BuilderManager: Connected peers: ", peer_count)
		
		# Only use RPC if we're a client OR if we're a server with connected clients
		# If we're the server with no clients, just call directly
		if peer != null and not (is_server and peer_count == 0):
			# In multiplayer, send RPC to server (or call locally if we are the server)
			print("BuilderManager: Using RPC")
			request_spawn_constructible.rpc_id(1, selected_part_id, ghost_instance.global_position, ghost_instance.global_rotation)
		else:
			# In single-player or local server with no clients, call directly
			print("BuilderManager: Calling directly (local)")
			request_spawn_constructible(selected_part_id, ghost_instance.global_position, ghost_instance.global_rotation)
	else:
		# Find parent under cursor
		# request_attach_component.rpc_id(1, parent_path, selected_part_id, attachment_node)
		pass

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
	constructible.register_component(comp)
	
	# Notify the simulation about the new entity
	var simulation = get_tree().current_scene
	if simulation and simulation.has_method("_on_multiplayer_spawner_spawned"):
		simulation._on_multiplayer_spawner_spawned(constructible)
		print("BuilderManager: Notified simulation of new entity")
	else:
		push_warning("BuilderManager: Could not find simulation to notify")
	
	# Ensure networking spawns it on clients (needs MultiplayerSpawner setup in scene)

@rpc("any_peer", "call_remote", "reliable")
func request_attach_component(parent_path: String, type: String, attachment_node_name: String):
	if not multiplayer.is_server(): return
	
	var parent = get_node(parent_path)
	if parent and parent is LCConstructible:
		var comp_scene = load(part_registry[type])
		var comp = comp_scene.instantiate()
		parent.add_child(comp)
		# Position at attachment node...
		parent.register_component(comp)

