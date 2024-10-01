extends Node

signal control_granted(peer_id, entity_path)
signal control_released(peer_id, entity_path)
signal control_request_denied(peer_id, entity_path)

var controlled_entities = {}  # {entity_path: controlling_peer_id}
var peer_controlled_entities = {}  # {peer_id: [entity_paths]}

func _ready():
	multiplayer.peer_connected.connect(_on_peer_connected)
	multiplayer.peer_disconnected.connect(_on_peer_disconnected)

func request_control(entity_path: NodePath, requester_id: int = multiplayer.get_unique_id()):
	if multiplayer.is_server():
		_process_control_request(requester_id, entity_path)
	else:
		rpc_id(1, "_server_process_control_request", entity_path)

func _process_control_request(peer_id: int, entity_path: NodePath):
	print("ControlManager: Processing control request from peer ", peer_id, " for entity ", entity_path)
	if entity_path in controlled_entities and controlled_entities[entity_path] != peer_id:
		print("ControlManager: Entity already controlled by another peer")
		_client_control_request_denied(peer_id, entity_path)
	else:
		if entity_path in controlled_entities:
			var previous_peer = controlled_entities[entity_path]
			if previous_peer != peer_id:
				print("ControlManager: Releasing control from previous peer")
				_release_control_internal(previous_peer, entity_path)

		print("ControlManager: Granting control to peer ", peer_id)
		controlled_entities[entity_path] = peer_id
		if peer_id not in peer_controlled_entities:
			peer_controlled_entities[peer_id] = []
		peer_controlled_entities[peer_id].append(entity_path)
		
		_client_control_granted(peer_id, entity_path)

@rpc("any_peer", "call_local")
func _server_process_control_request(entity_path: NodePath):
	var requester_id = multiplayer.get_remote_sender_id()
	_process_control_request(requester_id, entity_path)

func release_control(entity_path: NodePath):
	print("ControlManager: Releasing control for entity: ", entity_path)
	if multiplayer.is_server():
		_process_control_release(multiplayer.get_unique_id(), entity_path)
	else:
		rpc_id(1, "_server_process_control_release", entity_path)

func _process_control_release(peer_id: int, entity_path: NodePath):
	print("ControlManager: Processing control release from peer ", peer_id, " for entity ", entity_path)
	_release_control_internal(peer_id, entity_path)

@rpc("any_peer", "call_local")
func _server_process_control_release(entity_path: NodePath):
	var peer_id = multiplayer.get_remote_sender_id()
	_process_control_release(peer_id, entity_path)

func _release_control_internal(peer_id: int, entity_path: NodePath):
	if controlled_entities.get(entity_path) == peer_id:
		controlled_entities.erase(entity_path)
		peer_controlled_entities[peer_id].erase(entity_path)
		if peer_controlled_entities[peer_id].is_empty():
			peer_controlled_entities.erase(peer_id)
		_client_control_released(peer_id, entity_path)

func _client_control_granted(peer_id: int, entity_path: NodePath):
	print("ControlManager: Granting control to peer ", peer_id, " for entity ", entity_path)
	if multiplayer.is_server():
		_sync_client_control_granted.rpc(peer_id, entity_path)
	else:
		_sync_client_control_granted(peer_id, entity_path)

@rpc("authority", "reliable")
func _sync_client_control_granted(peer_id: int, entity_path: NodePath):
	print("ControlManager: Syncing control granted for peer ", peer_id, " and entity ", entity_path)
	if not multiplayer.is_server():
		controlled_entities[entity_path] = peer_id
		if peer_id not in peer_controlled_entities:
			peer_controlled_entities[peer_id] = []
		peer_controlled_entities[peer_id].append(entity_path)
	control_granted.emit(peer_id, entity_path)

func _client_control_released(peer_id: int, entity_path: NodePath):
	print("ControlManager: Control released from peer ", peer_id, " for entity ", entity_path)
	control_released.emit(peer_id, entity_path)

func _client_control_request_denied(peer_id: int, entity_path: NodePath):
	print("ControlManager: Control request denied for peer ", peer_id, " for entity ", entity_path)
	control_request_denied.emit(peer_id, entity_path)
	if multiplayer.is_server():
		rpc_id(peer_id, "_sync_client_control_request_denied", entity_path)

@rpc
func _sync_client_control_request_denied(entity_path: NodePath):
	print("ControlManager: Syncing control request denied for entity ", entity_path)
	control_request_denied.emit(multiplayer.get_unique_id(), entity_path)

func is_entity_controlled(entity_path: NodePath) -> bool:
	return entity_path in controlled_entities

func get_controlling_peer(entity_path: NodePath) -> int:
	return controlled_entities.get(entity_path, -1)

func get_controlled_entities(peer_id: int) -> Array:
	return peer_controlled_entities.get(peer_id, [])

func _on_peer_connected(id):
	if multiplayer.is_server():
		rpc_id(id, "_client_sync_control_state", controlled_entities)

func _on_peer_disconnected(id):
	if multiplayer.is_server():
		var entities_to_release = peer_controlled_entities.get(id, [])
		for entity_path in entities_to_release:
			_release_control_internal(id, entity_path)

@rpc
func _client_sync_control_state(server_controlled_entities):
	controlled_entities = server_controlled_entities
	peer_controlled_entities.clear()
	for entity_path in controlled_entities:
		var peer_id = controlled_entities[entity_path]
		if peer_id not in peer_controlled_entities:
			peer_controlled_entities[peer_id] = []
		peer_controlled_entities[peer_id].append(entity_path)
