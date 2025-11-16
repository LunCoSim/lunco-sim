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

# Load and setup TLS certificate for secure WSS connections
func setup_tls(cert_path: String, key_path: String) -> TLSOptions:
	var cert = X509Certificate.new()
	var key = CryptoKey.new()
	
	# Load certificate
	var cert_error = cert.load(cert_path)
	if cert_error != OK:
		push_error("Failed to load certificate from %s: %s" % [cert_path, error_string(cert_error)])
		return null
	
	# Load private key
	var key_error = key.load(key_path)
	if key_error != OK:
		push_error("Failed to load key from %s: %s" % [key_path, error_string(key_error)])
		return null
	
	# Create TLS options for server
	return TLSOptions.server(cert, key)

# Function to connect to a server with optional TLS
func connect_to_server(ip: String = "langrenus.lunco.space", port: int = 9000, tls: bool = false):
	if multiplayer.multiplayer_peer is WebSocketMultiplayerPeer:
		print("Already connected to a server")
		return

	peer = WebSocketMultiplayerPeer.new()

	var protocol = "wss://" if tls else "ws://"
	var connection_string = "%s%s:%d" % [protocol, ip, port]

	print("Connecting to server: ", connection_string)
	
	var error = peer.create_client(connection_string)
	if error != OK:
		push_error("Failed to create client: %s" % error_string(error))
		return
	
	multiplayer.multiplayer_peer = peer

func connect_to_local_server(tls: bool = false):
	connect_to_server("localhost", 9000, tls)


# Function to start hosting a server with optional TLS
func host(port: int = 9000, tls_cert_path: String = "", tls_key_path: String = ""):
	if multiplayer.multiplayer_peer is WebSocketMultiplayerPeer:
		print("Already hosting a server")
		return

	peer = WebSocketMultiplayerPeer.new()
	DisplayServer.window_set_title("Server")
	
	var server_tls_options: TLSOptions = null
	
	# Setup TLS if paths are provided
	if tls_cert_path and tls_key_path:
		server_tls_options = setup_tls(tls_cert_path, tls_key_path)
		if server_tls_options == null:
			push_error("Failed to setup TLS, hosting without TLS")
	
	print("Hosting on port %d, TLS enabled: %s" % [port, server_tls_options != null])
	
	var error = peer.create_server(port, "*", server_tls_options)
	if error != OK:
		push_error("Failed to create server: %s" % error_string(error))
		return
	
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
	if peer:
		print("Handshake headers: ", peer.handshake_headers)
	
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
