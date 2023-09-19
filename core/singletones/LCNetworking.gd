class_name LCNetworking
extends Node

var peer = ENetMultiplayerPeer.new()

## 
func connect_to_server(ip: String, port: int):
	peer.create_client(ip, port)
	multiplayer.multiplayer_peer = peer
	
func host(port: int = 9000):
	peer.create_server(9000)
	multiplayer.multiplayer_peer = peer
