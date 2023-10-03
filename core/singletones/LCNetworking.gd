# This is a class for networking 
class_name LCNetworking
extends Node

# Declaration of peer as MultiplayerPeer, which will be used to handle multiplayer networking
var peer: MultiplayerPeer

# A dictionary to store the connected players
var players = {}

# Initialization function
func _init():
	# Creating a new ENetMultiplayerPeer
	peer = ENetMultiplayerPeer.new()

func _ready():
	# Setting up signals to call relevant functions when a peer is connected or disconnected.
	multiplayer.peer_connected.connect(on_peer_connected)
	multiplayer.peer_disconnected.connect(on_peer_disconnected)

	# Setting up signals to call relevant functions upon server connection status changes.
	multiplayer.connection_failed.connect(on_server_connection_failed)
	multiplayer.connected_to_server.connect(on_server_connected)
	multiplayer.server_disconnected.connect(on_server_disconnected)

# Function to connect to a server
func connect_to_server(ip: String, port: int):
	# Creating a client
	peer.create_client(ip, port)
	# Assigning the peer to this multiplayer's peer
	multiplayer.multiplayer_peer = peer

# Function to start hosting a server
func host(port: int = 9000):
	# Creating a server
	peer.create_server(port)
	# Assigning the peer to this multiplayer's peer
	multiplayer.multiplayer_peer = peer


#---------------------------------------------------

# Function called when a peer connects
func on_peer_connected(id):
	# Adding the peer to players dictionary
	players[id] = {}

# Function called when a peer disconnects
func on_peer_disconnected(id):
	# Removing the peer from players dictionary
	players.erase(id)


# Function called when connection to server failed
func on_server_connection_failed():
	# This function currently does nothing.
	pass

# Function called when successfully connected to server.
func on_server_connected():
	# This function currently does nothing.
	pass

# Function called when server gets disconnected
func on_server_disconnected():
	# Printing a message to signal loss of server connection
	print("Lost connection to server")