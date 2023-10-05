extends Node


var entities = []

var entity_to_spawn = EntitiesDB.Entities.Astronaut

# Called when the node enters the scene tree for the first time.
func _ready():
	var menu = preload("res://widgets/menu/main_menu.tscn").instantiate()
	var win: PankuLynxWindow = Panku.windows_manager.create_window(menu)
	
	var size = menu.get_minimum_size() + Vector2(0, win._window_title_container.get_minimum_size().y)
	win.set_custom_minimum_size(size)
	win.size = win.get_minimum_size()

	win.set_window_title_text("Main menu")
	win.show_window()
	
	#PankuConsole.add
# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	#$Universe.position -= $Avatar.camera_global_position()
	#$Avatar.position = Vector3.ZERO
	pass


	
#------------------------------------------------------


@rpc("any_peer", "call_local")
func spawn(_entity: EntitiesDB.Entities, global_position=null): #TBD think of a class entity
	var id = multiplayer.get_remote_sender_id()
	print("spawn remoteid: ", id, " local id: ", multiplayer.get_unique_id(), " entity:", _entity)
	
	var entity = Entities.make_entity(_entity)
	
	if global_position != null:
		entity.position = %SpawnPosition.to_local(global_position)
	%SpawnPosition.add_child(entity, true)
	
	#return entity
	_on_multiplayer_spawner_spawned(entity)

func _on_multiplayer_spawner_spawned(entity):
	$Avatar.set_target(entity)

	
#----------
# Signals from Avatar

func _on_create_operator():
	spawn.rpc_id(1, EntitiesDB.Entities.Operator)

func _on_create_character():
	spawn.rpc_id(1, EntitiesDB.Entities.Character)

func _on_create_spacecraft():
	spawn.rpc_id(1, EntitiesDB.Entities.Spacecraft)

func _on_select_entity_to_spawn(entity_id =0):
	print("_on_select_entity_to_spawn", entity_id)
	var entity = EntitiesDB.Entities.keys()[entity_id]
	
	entity_to_spawn = entity_id
	
	pass
#---------------------------------------

# set avatars target for newly spawned entity
#func _on_multiplayer_spawner_spawned(node):
	#if node.name == str(multiplayer.get_unique_id()):
		#$Avatar.set_target(node)
#		


func _on_avatar_ray_cast(from: Vector3, to: Vector3):
	
	var space_state = $Universe.get_world_3d().direct_space_state
	

	var query = PhysicsRayQueryParameters3D.create(from, to)
	query.exclude = [self]
	var result = space_state.intersect_ray(query)
	
	if result:
		
		if result.collider is StaticBody3D:
			spawn.rpc_id(1, entity_to_spawn, result.position + Vector3(0, 1, 0))
		else:
			$Avatar.set_target(result.collider)
		
#			emit_signal("ray_hit", res["position"])
