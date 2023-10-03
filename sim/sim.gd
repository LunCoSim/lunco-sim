extends Node


var entities = []


# Called when the node enters the scene tree for the first time.
func _ready():
	var menu = preload("res://widgets/menu/main_menu.tscn").instantiate()
	var win: PankuLynxWindow = Panku.windows_manager.create_window(menu)
	win.size = menu.get_minimum_size() + win._window_title_container.get_minimum_size()
	win.set_window_title_text("Main menu")
	win.show_window()
	
# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	#$Universe.position -= $Avatar.camera_global_position()
	#$Avatar.position = Vector3.ZERO
	pass


	
#------------------------------------------------------


@rpc("any_peer", "call_local")
func spawn(_entity: EntitiesDB.Entities): #TBD think of a class entity
	var id = multiplayer.get_remote_sender_id()
	print("spawn remoteid: ", id, " local id: ", multiplayer.get_unique_id(), " entity:", _entity)
	
	var entity = Entities.make_entity(_entity)

	%SpawnPosition.add_child(entity, true)

	#_on_multiplayer_spawner_spawned(entity)


	
#----------
# Signals from Avatar

func _on_create_operator():
	spawn.rpc_id(1, EntitiesDB.Entities.Operator)

func _on_create_character():
	spawn.rpc_id(1, EntitiesDB.Entities.Character)

func _on_create_spacecraft():
	spawn.rpc_id(1, EntitiesDB.Entities.Spacecraft)
	
#---------------------------------------

# set avatars target for newly spawned entity
#func _on_multiplayer_spawner_spawned(node):
	#if node.name == str(multiplayer.get_unique_id()):
		#$Avatar.set_target(node)
#		


func _on_avatar_ray_cast(from: Vector3, to: Vector3):
	
	var space_state = $Universe.get_world_3d().direct_space_state
	
	#tbd, probably because of multithreading
#	if space_state:
	var query = PhysicsRayQueryParameters3D.create(from, to)
	query.exclude = [self]
	var result = space_state.intersect_ray(query)
	
	if result:
		
		if result.collider is StaticBody3D:
			var starship= Entities.make_entity(EntitiesDB.Entities.Spacecraft)
			starship.position = $Universe.to_local(result.position) + Vector3(0, 1, 0)

			$Universe.add_child(starship)
		else:
			$Avatar.set_target(result.collider)
		
		
		print(" Selected: ", result)
#			emit_signal("ray_hit", res["position"])
