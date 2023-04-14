extends Node

var PlayerEntity := preload("res://core/entities/player-entity.tscn")
var OperatorEntity := preload("res://core/entities/operator-entity.tscn")
var SpacecraftEntity := preload("res://core/entities/starship-entity.tscn")


var players = {}
var entities = []

# Called when the node enters the scene tree for the first time.
func _ready():
	if multiplayer.is_server():
		%MachineRole.text = "Server"
		
		multiplayer.peer_connected.connect(on_peer_connected)
		multiplayer.peer_disconnected.connect(on_peer_disconnected)
	else:
		%MachineRole.text = "Peer id: " + str(multiplayer.get_unique_id())
		
		multiplayer.connection_failed.connect(on_server_connection_failed)
		multiplayer.connected_to_server.connect(on_server_connected)
		multiplayer.server_disconnected.connect(on_server_disconnected)
	
	

# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass

## Signal processing

#------------------------------------------------------

func on_peer_connected(id):
	print("player connected: ", id)
	players[id] = {}

func on_peer_disconnected(id):
	print("player removed: ", id)
	
	players.erase(id)
	
	for entity in %SpawnPosition.get_children():
		if entity.name.to_int() == id:
			entity.queue_free()

func on_server_connection_failed():
	pass
	
func on_server_connected():
	pass
	
func on_server_disconnected():
	print("Lost connection to server")
	
#------------------------------------------------------
	
@rpc("call_local", "any_peer")
func send_message(player_name, message, is_server):
	pass

@rpc("any_peer")
func add_player(id):
	pass
#	var player_instance = player.instantiate()
#	player_instance.name = str(id)
#	%SpawnPosition.add_child(player_instance)

#	send_message.rpc(str(id), " has joined the game", false)

@rpc("any_peer", "call_local")
func spawn(_entity: EntitiesDB.Entities): #TBD think of a class entity
	var id = multiplayer.get_remote_sender_id()
	print("add_operator remoteid: ", id, " local id: ", multiplayer.get_unique_id())
	
	var found := false
	
	for i in %SpawnPosition.get_children():
		if i.name == str(id):
			found = true
	
	if not found:
		var entity = Entities.make_entity(_entity)
		entity.name = str(id)

		%SpawnPosition.add_child(entity)

		_on_multiplayer_spawner_spawned(entity)

		send_message.rpc(str(id), " has joined the game", false)

	
#----------
# Signals from Avatar

func _on_create_operator():
	spawn.rpc_id(1, EntitiesDB.Entities.Operator)

func _on_create_character():
	spawn.rpc_id(1, EntitiesDB.Entities.Character)

func _on_create_spacecraft():
	spawn.rpc_id(1, EntitiesDB.Entities.Spacecraft)
	
#---------------------------------------

func _on_multiplayer_spawner_spawned(node):
	if node.name == str(multiplayer.get_unique_id()):
		$Avatar.set_target(node)
#		


func _on_avatar_ray_cast(from: Vector3, to: Vector3):
	
	var space_state = $World.get_world_3d().direct_space_state
	
	var query = PhysicsRayQueryParameters3D.create(from, to)
	query.exclude = [self]
	var result = space_state.intersect_ray(query)
	
	
	if result:
		print(" Selected: ", result)
#			emit_signal("ray_hit", res["position"])
pass # Replace with function body.
