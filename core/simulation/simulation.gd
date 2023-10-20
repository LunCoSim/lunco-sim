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

#--------------------------------
@export var entities = []
@export var spawn_node: Node3D

var owners = {}

#
#func _init():
	
# Called when the node enters the scene tree for the first time.
func _ready():
	Panku.gd_exprenv.register_env("Avatar", $Avatar)
	print("Main ready")
	print(OS.get_cmdline_args())
	## TBD Move to separate file, as new modes like chat-server are appearing
	if "--server" in OS.get_cmdline_args():
		print("Headless running")
		LCNet.host()
	multiplayer.peer_disconnected.connect(_on_peer_disconnected)
	
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
		var entity = Entities.make_entity(_entity)
		if entity != null:
			if global_position != null:
				entity.position = spawn_node.to_local(global_position)
			else:
				entity.position = spawn_node.global_position
			
			spawn_node.add_child(entity, true)
			
			_on_multiplayer_spawner_spawned(entity)

@rpc("any_peer", "call_local", "reliable")
func set_authority(path, owner):
	var node = get_tree().get_root().get_node(path)
	node.set_multiplayer_authority(owner)

@rpc("any_peer", "call_local", "reliable")
func requesting_control(path):
	var owner = multiplayer.get_remote_sender_id()
	if multiplayer.is_server():
		var _owner = owners.get(path) 
			
		if _owner == null: # TBD: Access control
			owners[path] = owner
			set_authority.rpc(path, owner)
			control_granted_notify.rpc_id(owner, path)
		else:
			control_declined_notify.rpc_id(owner, path)

@rpc("any_peer", "call_local", "reliable")
func release_control(path):
	if multiplayer.is_server():
		owners[path] = null 
		set_authority.rpc(path, 1)

#---------------------------------------
# Notifying about changed state
@rpc("any_peer", "call_local", "reliable")
func control_granted_notify(path):
	control_granted.emit(path)

@rpc("any_peer", "call_local", "reliable")
func control_declined_notify(path):
	control_declined.emit(path)

#---------------------------------------

func _on_multiplayer_spawner_spawned(entity):	
	entities.append(entity)
	Panku.notify("%s created" % entity.name)
	entity_spawned.emit(entity)
	entities_updated.emit(entities)
	
	#TBD It's done for debug, should be done somewhere else, maybe special debug
	#node? Maybe it should be global? Should be as reaction on entity_spawned
	
	Panku.gd_exprenv.register_env(entity.name, entity)

#---------------------------------------
# Signals from Avatar

func _on_select_entity_to_spawn(entity_id=0, position=null):
	spawn.rpc_id(1, entity_id, position) #Spawning on server

func _on_peer_disconnected(peer_id):
	if multiplayer.is_server(): # Cleaning autority
		for key in owners:
			if owners[key] == peer_id:
				release_control(key)

func _on_avatar_requesting_control(entity_idx): #TBD: To path
	if entity_idx < entities.size():
		requesting_control.rpc_id(1, entities[entity_idx].get_path())

func _on_avatar_release_control(path):
	release_control.rpc_id(1, path)
