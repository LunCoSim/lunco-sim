@icon("res://sim/simulation.svg")
class_name LCSimulation
extends Node

@export var entities = []

# Called when the node enters the scene tree for the first time.
func _ready():
	Panku.gd_exprenv.register_env("Avatar", $Avatar)
	
# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
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
	entities.append(entity)
	
	if global_position != null:
		entity.position = %SpawnPosition.to_local(global_position)
	%SpawnPosition.add_child(entity, true)
	
	#return entity
	_on_multiplayer_spawner_spawned(entity)

func _on_multiplayer_spawner_spawned(entity):
	$Avatar.update_entities(entities)
	$Avatar.set_target(entity)
	var num = %SpawnPosition.get_child_count()
	Panku.gd_exprenv.register_env("Entity"+str(num), entity)

	
#----------
# Signals from Avatar

func _on_create_operator():
	spawn.rpc_id(1, EntitiesDB.Entities.Operator)

func _on_create_character():
	spawn.rpc_id(1, EntitiesDB.Entities.Gobot)

func _on_create_spacecraft():
	spawn.rpc_id(1, EntitiesDB.Entities.Spacecraft)

func _on_select_entity_to_spawn(entity_id=0, position=Vector3.ZERO):
	spawn.rpc_id(1, entity_id, position)
	
#---------------------------------------
