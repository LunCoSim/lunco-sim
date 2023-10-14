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

# Called when the node enters the scene tree for the first time.
func _ready():
	Panku.gd_exprenv.register_env("Avatar", $Avatar)
	#if spawn_node:
		#$MultiplayerSpawner.spawn_path = spawn_node
	
# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(_delta):
		#Origin shifting. TBD how to do it in multiplayer
	if Engine.get_process_frames() % 100:
		var pos: Vector3 = $Avatar.camera_global_position()
		if pos.length_squared() > 1000*1000: # Doing origin shifing  if far away to prevent jutter
			%Universe.position -= $Avatar.camera_global_position()

#------------------------------------------------------

@rpc("any_peer", "call_local")
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
			
		spawn_node.add_child(entity, true)
		
		Panku.notify("%s created" % entity.name)
		entity_spawned.emit(entity)
		entities_updated.emit(entities)
		
		#TBD It's done for debug, should be done somewhere else, maybe special debug
		#node? Maybe it should be global?
		var num = spawn_node.get_child_count()
		Panku.gd_exprenv.register_env("Entity"+str(num), entity)

@rpc("any_peer", "call_local")
func requesting_control(target):
	var owner = owners.get(target) 
	if owner == null:
		owners[owner] = multiplayer.get_remote_sender_id()
		
		if target is LCController:
			target.set_authority(multiplayer.get_unique_id())
		else:
			target.set_multiplayer_authority(multiplayer.get_unique_id())
		
		control_granted.emit(target)
	else:
		control_declined.emit(target)

@rpc("any_peer", "call_local")
func release_control(target):
	owners[target] = null
	if target is LCController:
		target.set_authority.rpc(1)
	else:
		target.set_multiplayer_authority.rpc(1)
	
func _on_multiplayer_spawner_spawned(node):
	entities.append(node)
	entities_updated.emit(entities)

#----------
# Signals from Avatar

func _on_select_entity_to_spawn(entity_id=0, position=null):
	spawn.rpc_id(1, entity_id, position)


func _on_avatar_requesting_control(target):
	requesting_control.rpc_id(1, target)
	

#---------------------------------------


func _on_avatar_release_control(target):
	release_control.rpc_id(1, target)
