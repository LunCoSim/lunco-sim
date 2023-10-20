## This class is the root node responsible for storing simulation state,
## spawning entities and so on

@icon("res://core/simulation/simulation.svg")
class_name LCSimulation
extends Node

#--------------------------------
signal entities_updated(entities)
signal entity_spawned()

signal control_granted(entity)
signal control_declined(entity)

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
	
	
	multiplayer.peer_disconnected.connect(on_peer_disconnected)
	#if spawn_node:
		#$MultiplayerSpawner.spawn_path = spawn_node
	
## Called every frame. 'delta' is the elapsed time since the previous frame.
#func _process(_delta):
		##Origin shifting. TBD how to do it in multiplayer
	#if Engine.get_process_frames() % 100:
		#var pos: Vector3 = $Avatar.camera_global_position()
		#if pos.length_squared() > 1000*1000: # Doing origin shifing  if far away to prevent jutter
			#%Universe.position -= $Avatar.camera_global_position()

#------------------------------------------------------

## TBD: That's a factory method
@rpc("any_peer", "call_local", "reliable")
func spawn(_entity: EntitiesDB.Entities, global_position=null): #TBD think of a class entity
	var id = multiplayer.get_remote_sender_id()
	print("spawn remoteid: ", id, " local id: ", multiplayer.get_unique_id(), " entity:", _entity)
	
	var entity = Entities.make_entity(_entity)
	if entity != null:
		entities.append(entity)
		
		if global_position != null:
			entity.position = spawn_node.to_local(global_position)
		else:
			entity.position = spawn_node.global_position
		
		var controller = LCController.find_controller(entity)
		if controller != null:
			init_controller_signals(controller)
			
		spawn_node.add_child(entity, true)
		
		Panku.notify("%s created" % entity.name)
		entity_spawned.emit(entity)
		entities_updated.emit(entities)
		
		#TBD It's done for debug, should be done somewhere else, maybe special debug
		#node? Maybe it should be global?
		
		Panku.gd_exprenv.register_env(entity.name, entity)

func init_controller_signals(controller):
	controller.requesting_controller_authority.connect(requesting_control)
	controller.releasing_controller_authority.connect(release_control)
	
	controller.control_granted_n.connect(_on_control_granted)
	controller.control_declined_n.connect(_on_control_declined)
	
	
func _on_control_granted(controller, owner):
	controller.set_authority.rpc(multiplayer.get_unique_id())
	control_granted.emit(controller)

func _on_control_declined(controller, owner):
	control_declined.emit(controller)
	
func requesting_control(target, owner):
	print(target, owner)
	var _owner = owners.get(target) 
		
	if _owner == null:
		owners[target] = owner
		
		if target is LCController:
			set_authority.rpc(target.get_parent().get_path(), owner)
			#target.set_authority.rpc(owner)
			target.control_granted_notify.rpc_id(owner)
	else:
		if target is LCController:
			target.control_declined_notify.rpc_id(owner)
			
@rpc("any_peer", "call_local", "reliable")
func set_authority(path, owner):
	var node = get_tree().get_root().get_node(path)
	print("Setting authority ", multiplayer.get_remote_sender_id()  )
	print("Setting authority owner ", owner )
	node.set_multiplayer_authority(owner)
	
func release_control(target, owner):
	owners[target] = null 
	
	if target is LCController:
		set_authority.rpc(target.get_parent().path(), 1)
	
func _on_multiplayer_spawner_spawned(node):
	
	var controller: = LCController.find_controller(node)
	
	if controller:
		init_controller_signals(controller)
	
	entities.append(node)
	entities_updated.emit(entities)

#----------
# Signals from Avatar

func _on_select_entity_to_spawn(entity_id=0, position=null):
	spawn.rpc_id(1, entity_id, position)

func on_peer_disconnected(peer_id):
	if multiplayer.is_server():
		for key in owners:
			var owner:LCController = owners[key]
			
			if owner == peer_id:
				owners[key] = null
				key.set_authority.rpc(1)
