# This is a class for networking 
class_name LCNetworking
extends Node

# Declaration of peer as MultiplayerPeer, which will be used to handle multiplayer networking
var peer: WebSocketMultiplayerPeer

# A dictionary to store the connected players
var players = {}

func _ready():
	# Setting up signals to call relevant functions when a peer is connected or disconnected.
	multiplayer.peer_connected.connect(on_peer_connected)
	multiplayer.peer_disconnected.connect(on_peer_disconnected)

	# Setting up signals to call relevant functions upon server connection status changes.
	multiplayer.connection_failed.connect(on_server_connection_failed)
	multiplayer.connected_to_server.connect(on_server_connected)
	multiplayer.server_disconnected.connect(on_server_disconnected)
	
	Profile.profile_changed.connect(_on_profile_changed)
	
# Function to connect to a server
func connect_to_server(ip: String="langrenus.lunco.space", port: int = 9000, tls: bool = false):
	if not multiplayer.multiplayer_peer is WebSocketMultiplayerPeer:
		peer = WebSocketMultiplayerPeer.new() # Already connected
	else:
		return

	var protocol = "wss://" if tls else "ws://"
	var connection_string = "%s%s:%d" % [protocol, ip, port]


	# Creating a client
	Logger.info("Connecting to server: ", connection_string )
	print("Connecting to server: ", connection_string)


	peer.create_client(connection_string)
	multiplayer.multiplayer_peer = peer
	# Assigning the peer to this multiplayer's peer

func connect_to_local_server():
	connect_to_server("127.0.0.1", 9000)

# Function to start hosting a server
func host(port: int = 9000, tls_options: TLSOptions = null):
	if not multiplayer.multiplayer_peer is WebSocketMultiplayerPeer:
		peer = WebSocketMultiplayerPeer.new() # Already connected
	else:
		return

	# Creating a server
	
	Logger.info("Hosting on %d" % port)
	DisplayServer.window_set_title("Server")
	
	print("Hosting on %d, tls_options: " % port, tls_options)
	peer.create_server(port, "*",tls_options)
	multiplayer.multiplayer_peer = peer

#---------------------------------------------------
@rpc("any_peer", "call_remote", "reliable")
func update_player_info(_username, _userwallet):
	var id = multiplayer.get_remote_sender_id()
	
	players[id] = {
		"username": _username,
		"wallet": _userwallet
	}
	
	Users._on_user_connected(id, _username, _userwallet)
	
#---------------------------------------------------

# Function called when a peer connects
func on_peer_connected(id):
	print("on_peer_connected: ", id)
	# Adding the peer to players dictionary
	players[id] = {}
	update_player_info.rpc_id(id, Profile.username, Profile.wallet)
	
	# Update the Users singleton
	Users._on_user_connected(id, "", "")  # We'll update the username and wallet later

func _on_profile_changed():
	update_player_info.rpc(Profile.username, Profile.wallet)
	
# Function called when a peer disconnects
func on_peer_disconnected(id):
	print("on_peer_disconnected: ", id)
	# Removing the peer from players dictionary
	players.erase(id)
	
	# Update the Users singleton
	Users._on_user_disconnected(id)


# Function called when connection to server failed
func on_server_connection_failed():
	print("on_server_connection_failed")
	print(peer.get_packet_error())
	# This function currently does nothing.
	pass
	
# Function called when successfully connected to server.
func on_server_connected():
	print("on_server_connected")
	DisplayServer.window_set_title("Connected to server")
	# This function currently does nothing.
	pass

# Function called when server gets disconnected
func on_server_disconnected():
	# Printing a message to signal loss of server connection
	print("Lost connection to server")
	DisplayServer.window_set_title("Lost connection to server")
