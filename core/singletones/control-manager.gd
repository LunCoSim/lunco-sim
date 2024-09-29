extends Node

signal control_granted(peer_id, entity_path)
signal control_released(peer_id, entity_path)
signal control_request_denied(peer_id, entity_path)

var controlled_entities = {}  # {entity_path: controlling_peer_id}

func request_control(peer_id: int, entity_path: NodePath):
	if multiplayer.is_server():
		_process_control_request(peer_id, entity_path)
	else:
		rpc_id(1, "_process_control_request", peer_id, entity_path)

@rpc("any_peer", "call_local")
func _process_control_request(peer_id: int, entity_path: NodePath):
	if not multiplayer.is_server():
		return

	if entity_path in controlled_entities:
		control_request_denied.emit(peer_id, entity_path)
		rpc_id(peer_id, "_on_control_request_denied", entity_path)
	else:
		controlled_entities[entity_path] = peer_id
		control_granted.emit(peer_id, entity_path)
		rpc("_on_control_granted", peer_id, entity_path)

func release_control(peer_id: int, entity_path: NodePath):
	if multiplayer.is_server():
		_process_control_release(peer_id, entity_path)
	else:
		rpc_id(1, "_process_control_release", peer_id, entity_path)

@rpc("any_peer", "call_local")
func _process_control_release(peer_id: int, entity_path: NodePath):
	if not multiplayer.is_server():
		return

	if controlled_entities.get(entity_path) == peer_id:
		controlled_entities.erase(entity_path)
		control_released.emit(peer_id, entity_path)
		rpc("_on_control_released", peer_id, entity_path)

@rpc
func _on_control_granted(peer_id: int, entity_path: NodePath):
	control_granted.emit(peer_id, entity_path)

@rpc
func _on_control_released(peer_id: int, entity_path: NodePath):
	control_released.emit(peer_id, entity_path)

@rpc
func _on_control_request_denied(entity_path: NodePath):
	control_request_denied.emit(multiplayer.get_unique_id(), entity_path)

func is_entity_controlled(entity_path: NodePath) -> bool:
	return entity_path in controlled_entities

func get_controlling_peer(entity_path: NodePath) -> int:
	return controlled_entities.get(entity_path, -1)
