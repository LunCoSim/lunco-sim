## This class is the root node responsible for storing simulation state,
## spawning entities and so on

@icon("res://core/simulation/simulation.svg")
class_name LCSimulation
extends Node

#--------------------------------
signal entities_updated(entities)
signal entity_spawned()

signal control_granted(path)
signal control_declined(path)
signal control_released(path)

#--------------------------------
@export var entities = []
@export var spawn_node: Node3D

var owners = {}

#
#func _init():
	
# Called when the node enters the scene tree for the first time.
func _ready():

	# Parse command line arguments
	var arguments = OS.get_cmdline_args()
	var certificate_path = ""
	var key_path = ""

	#--------------------------------

	print("Simulation _ready, arguments: ", arguments)

	#Certificate and key check for wss
	for i in range(arguments.size()):
		match arguments[i]:
			"--certificate":
				if i + 1 < arguments.size():
					certificate_path = arguments[i + 1]
			"--key":
				if i + 1 < arguments.size():
					key_path = arguments[i + 1]
	
	# Start the server
	var use_ssl = certificate_path != "" and key_path != ""

	#-----------------------------
	if "--server" in OS.get_cmdline_args():
		print("Server running")

		if use_ssl:
			print("Starting secure server with SSL certificates:")
			print("Certificate: ", certificate_path)
			print("Private Key: ", key_path)
			LCNet.host(9000, certificate_path, key_path)
		else:
			print("Starting server without SSL")
			LCNet.host()

	elif "--connect" in OS.get_cmdline_args():
		# Wait for 2 seconds, then connect
		get_tree().create_timer(2.0).timeout.connect(LCNet.connect_to_local_server)

	multiplayer.peer_disconnected.connect(_on_peer_disconnected)
	ControlManager.control_granted.connect(_on_control_granted)
	ControlManager.control_released.connect(_on_control_released)
	ControlManager.control_request_denied.connect(_on_control_request_denied)
	
	# Show tutorial only if user hasn't disabled it
	if not Profile.hide_tutorial:
		LCWindows.show_tutorial()
	# Main menu will be shown on demand with ESC key
	
## Called every frame. 'delta' is the elapsed time since the previous frame.
#func _process(_delta):
		##Origin shifting. TBD how to do it in multiplayer
	#if Engine.get_process_frames() % 100:
		#var pos: Vector3 = $Avatar.camera_global_position()
		#if pos.length_squared() > 1000*1000: # Doing origin shifing  if far away to prevent jutter
			#%Universe.position -= $Avatar.camera_global_position()

#------------------------------------------------------
# RPC
## TBD: That's a factory method that spawns entities
@rpc("any_peer", "call_local", "reliable")
func spawn(_entity: EntitiesDB.Entities, global_position=null): #TBD think of a class entity
	if multiplayer.is_server():
		var entity = EntitiesDB.make_entity(_entity)
		if entity != null:
			if global_position != null:
				entity.position = spawn_node.to_local(global_position)
			else:
				entity.position = spawn_node.global_position
			
			spawn_node.add_child(entity, true)
			
			_on_multiplayer_spawner_spawned(entity)

@rpc("any_peer", "call_local", "reliable")
func set_authority(path, _owner):
	var node = get_tree().get_root().get_node(path)
	node.set_multiplayer_authority(_owner)
	entities_updated.emit(entities)

@rpc("any_peer", "call_local", "reliable")
func requesting_control(path):
	var remote_id = multiplayer.get_remote_sender_id()
	if multiplayer.is_server():
		var _owner = owners.get(path) 
			
		if _owner == null: # TBD: Access control
			owners[path] = remote_id
			set_authority.rpc(path, remote_id)
			control_granted_notify.rpc_id(remote_id, path)
		else:
			if _owner == multiplayer.get_remote_sender_id():
				release_control(path)
			else:
				control_declined_notify.rpc_id(remote_id, path)

# Add this new method to handle entity index-based control requests
func request_control_by_index(entity_idx):
	var requester_id = multiplayer.get_remote_sender_id()
	if requester_id == 0:
		requester_id = multiplayer.get_unique_id()
	print("Simulation received control request for entity index/node: ", entity_idx, " from peer: ", requester_id)
	
	if typeof(entity_idx) == TYPE_INT:
		if entity_idx < entities.size():
			var entity = entities[entity_idx]
			print("Requesting control for entity: ", entity.name)
			ControlManager.request_control(entity.get_path(), requester_id)
		else:
			print("Invalid entity index: ", entity_idx)
	elif entity_idx is Node:
		print("Requesting control for entity node: ", entity_idx.name)
		ControlManager.request_control(entity_idx.get_path(), requester_id)
	else:
		print("Invalid entity identifier type: ", typeof(entity_idx))

@rpc("any_peer", "call_local", "reliable")
func release_control(path):
	ControlManager.release_control(path)

#---------------------------------------
# Notifying about changed state
@rpc("any_peer", "call_local", "reliable")
func control_granted_notify(path):
	control_granted.emit(path)

@rpc("any_peer", "call_local", "reliable")
func control_declined_notify(path):
	control_declined.emit(path)

@rpc("any_peer", "call_local", "reliable")
func control_released_notify(path):
	control_released.emit(path)
	
func _on_control_granted(peer_id, path):
	print("Simulation: Control granted for entity: ", path)

func _on_control_released(peer_id, entity_path: NodePath):
	print("Simulation: Control released for entity: ", entity_path)

func _on_control_request_denied(peer_id, entity_path: NodePath):
	print("Simulation: Control declined for entity: ", entity_path)

@rpc("any_peer")
func _on_avatar_requesting_control(entity_idx):
	request_control_by_index(entity_idx)

func _on_avatar_release_control(path: NodePath):
	var releaser_id = multiplayer.get_remote_sender_id()
	ControlManager.release_control(path)

#---------------------------------------
# Commands for LCCommandExecutor

func cmd_spawn(args: Dictionary) -> String:
	var type_name = args.get("type", "")
	var entity_type = -1
	
	if type_name is String:
		# Try to match enum name
		for key in EntitiesDB.Entities.keys():
			if key.to_lower() == type_name.to_lower():
				entity_type = EntitiesDB.Entities[key]
				break
	elif type_name is float or type_name is int:
		entity_type = int(type_name)
		
	if entity_type == -1:
		return "Unknown entity type: %s" % str(type_name)
		
	var global_pos = null
	if args.has("position"):
		var p = args["position"]
		if p is Array and p.size() >= 3:
			global_pos = Vector3(p[0], p[1], p[2])
		elif p is Vector3:
			global_pos = p
			
	spawn.rpc_id(1, entity_type, global_pos)
	return "Spawned %s" % EntitiesDB.Entities.keys()[entity_type]

func cmd_list_entities(_args: Dictionary) -> Array:
	return EntitiesDB.Entities.keys()

#---------------------------------------

func _on_multiplayer_spawner_spawned(entity):	
	entities.append(entity)
	entity_spawned.emit(entity)
	entities_updated.emit(entities)
	
	#TBD It's done for debug, should be done somewhere else, maybe special debug
	#node? Maybe it should be global? Should be as reaction on entity_spawned

#---------------------------------------
# Signals from Avatar

func _on_select_entity_to_spawn(entity_id=0, position=null):
	spawn.rpc_id(1, entity_id, position) #Spawning on server

func _on_peer_disconnected(peer_id):
	print("Peer disconnected: ", peer_id)
	if multiplayer.is_server(): # Cleaning authority
		for key in owners.keys():
			if owners[key] == peer_id:
				print("Releasing control for ", key, " from disconnected peer ", peer_id)
				release_control(key)
