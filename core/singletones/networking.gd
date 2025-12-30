# This is a class for networking 
class_name LCNetworking
extends Node

# Signal emitted when connection state changes
signal connection_state_changed(state: String)

# A dictionary to store the connected players
var players = {}

# Connection state tracking
var connection_state: String = "disconnected"

# Reconnection settings
var reconnect_delay: float = 5.0

var reconnect_timer: Timer = null
var last_connection_ip: String = ""
var last_connection_port: int = 9000
var last_connection_tls: bool = true

func _ready():
	# Setting up signals to call relevant functions when a peer is connected or disconnected.
	multiplayer.peer_connected.connect(on_peer_connected)
	multiplayer.peer_disconnected.connect(on_peer_disconnected)

var server_version: String = ""
var server_git_hash: String = ""
signal server_version_received(version: String, git_hash: String)

@rpc("authority", "call_remote", "reliable")
func receive_server_version(ver: String, git_hash: String):
	print("Received server version: ", ver, " hash: ", git_hash)
	server_version = ver
	server_git_hash = git_hash
	server_version_received.emit(ver, git_hash)

func _send_version_to_peer(peer_id: int):
	var local_ver = str(ProjectSettings.get_setting("application/config/version"))
	var local_hash = LCVersionHelper.get_git_hash() # Now using the singleton
	receive_server_version.rpc_id(peer_id, local_ver, local_hash)


	# Setting up signals to call relevant functions upon server connection status changes.
	multiplayer.connection_failed.connect(on_server_connection_failed)
	multiplayer.connected_to_server.connect(on_server_connected)
	multiplayer.server_disconnected.connect(on_server_disconnected)
	
	Profile.profile_changed.connect(_on_profile_changed)
	
	# Setup reconnection timer
	_setup_reconnect_timer()
	
	# Auto-connect if running on alpha.lunco.space
	_check_auto_connect()

# Load and setup TLS certificate for secure WSS connections
func setup_tls(cert_path: String, key_path: String) -> TLSOptions:
	var cert = X509Certificate.new()
	var key = CryptoKey.new()
	
	# Convert res:// paths to filesystem paths
	# X509Certificate.load() and CryptoKey.load() expect filesystem paths, not resource paths
	var cert_fs_path = cert_path
	var key_fs_path = key_path
	
	if cert_path.begins_with("res://"):
		cert_fs_path = ProjectSettings.globalize_path(cert_path)
		print("Converted certificate path: %s -> %s" % [cert_path, cert_fs_path])
	
	if key_path.begins_with("res://"):
		key_fs_path = ProjectSettings.globalize_path(key_path)
		print("Converted key path: %s -> %s" % [key_path, key_fs_path])
	
	# Verify files exist before attempting to load
	if not FileAccess.file_exists(cert_fs_path):
		push_error("Certificate file does not exist: %s" % cert_fs_path)
		return null
	
	if not FileAccess.file_exists(key_fs_path):
		push_error("Private key file does not exist: %s" % key_fs_path)
		return null
	
	print("Certificate file exists: %s" % cert_fs_path)
	print("Private key file exists: %s" % key_fs_path)
	
	# Load certificate
	print("Attempting to load certificate...")
	var cert_error = cert.load(cert_fs_path)
	if cert_error != OK:
		push_error("Failed to load certificate from %s: Error code %d (%s)" % [cert_fs_path, cert_error, error_string(cert_error)])
		return null
	print("Certificate loaded successfully")
	
	# Load private key
	print("Attempting to load private key...")
	var key_error = key.load(key_fs_path)
	if key_error != OK:
		push_error("Failed to load key from %s: Error code %d (%s)" % [key_fs_path, key_error, error_string(key_error)])
		return null
	print("Private key loaded successfully")
	
	# Create TLS options for server
	# NOTE: TLSOptions.server() signature is: server(key: CryptoKey, cert: X509Certificate)
	# The key comes FIRST, then the certificate
	print("Creating TLS options...")
	var tls_options = TLSOptions.server(key, cert)
	print("TLS options created successfully")
	return tls_options

# Function to connect to a server with optional TLS
func connect_to_server(ip: String = "langrenus.lunco.space", port: int = 9000, tls: bool = true):
	if multiplayer.multiplayer_peer is WebSocketMultiplayerPeer:
		print("Already connected to a server")
		return

	# Save connection parameters for reconnection
	last_connection_ip = ip
	last_connection_port = port
	last_connection_tls = tls

	var ws_peer = WebSocketMultiplayerPeer.new()

	# Automatically disable TLS for localhost connections
	# SSL certificates don't work with IP addresses, only domain names
	var is_localhost = ip in ["localhost", "127.0.0.1", "::1"]
	if is_localhost and tls:
		print("Localhost detected (%s), disabling TLS (using ws:// instead of wss://)" % ip)
		tls = false
		last_connection_tls = false

	var protocol = "wss://" if tls else "ws://"
	var connection_string = "%s%s:%d" % [protocol, ip, port]

	print("Connecting to server: ", connection_string)
	_set_connection_state("connecting")
	
	var error = ws_peer.create_client(connection_string)
	if error != OK:
		push_error("Failed to create client: %s" % error_string(error))
		_set_connection_state("failed")
		return
	
	multiplayer.multiplayer_peer = ws_peer

func connect_to_local_server(tls: bool = false):
	connect_to_server("localhost", 9000, tls)


# Function to start hosting a server with optional TLS
func host(port: int = 9000, tls_cert_path: String = "", tls_key_path: String = ""):
	if multiplayer.multiplayer_peer is WebSocketMultiplayerPeer:
		print("Already hosting a server")
		return

	var ws_peer = WebSocketMultiplayerPeer.new()
	DisplayServer.window_set_title("Server")
	
	var server_tls_options: TLSOptions = null
	
	# Setup TLS if paths are provided
	if tls_cert_path and tls_key_path:
		print("Setting up TLS with cert: %s, key: %s" % [tls_cert_path, tls_key_path])
		server_tls_options = setup_tls(tls_cert_path, tls_key_path)
		if server_tls_options == null:
			push_error("Failed to setup TLS, hosting without TLS")
		else:
			print("TLS options object created: %s" % server_tls_options)
	
	print("Hosting on port %d, TLS enabled: %s" % [port, server_tls_options != null])
	print("About to call create_server with TLS options: %s" % server_tls_options)
	
	var error = ws_peer.create_server(port, "*", server_tls_options)
	if error != OK:
		push_error("Failed to create server: %s" % error_string(error))
		return
	
	print("Server created successfully, error code: %d" % error)
	multiplayer.multiplayer_peer = ws_peer

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
	
	if multiplayer.is_server():
		_send_version_to_peer(id)

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

	var ws_peer = multiplayer.multiplayer_peer as WebSocketMultiplayerPeer
	if ws_peer:
		print("Handshake headers: ", ws_peer.handshake_headers)

		# Get underlying connection status for more details
		var status = ws_peer.get_connection_status()
		print("Peer connection status: ", status)

		# Check if there were any specific WebSocket errors
		if ws_peer.has_method("get_requested_url"):
			print("Requested URL: ", ws_peer.get_requested_url())

		# Show supported sub-protocols if any
		if ws_peer.has_method("get_supported_protocols"):
			print("Supported protocols: ", ws_peer.get_supported_protocols())

	print("Possible causes:")
	print("- Server not running on port 9000")
	print("- SSL/TLS certificate issues (check server logs)")
	print("- Network firewall blocking connection")
	print("- Incorrect URL or port")
	
	# Reset the peer so we can try connecting again
	multiplayer.multiplayer_peer = null
	_set_connection_state("failed")
	
	# Attempt reconnection if enabled
	if Profile.auto_reconnect:
		_start_reconnect_timer()
	
# Function called when successfully connected to server.
func on_server_connected():
	print("on_server_connected")
	DisplayServer.window_set_title("Connected to server")
	_set_connection_state("connected")
	
	# Stop reconnection timer if it's running
	if reconnect_timer and reconnect_timer.is_stopped() == false:
		reconnect_timer.stop()

# Function called when server gets disconnected
func on_server_disconnected():
	# Printing a message to signal loss of server connection
	print("Lost connection to server")
	DisplayServer.window_set_title("Lost connection to server")
	
	# Reset the peer so we can try connecting again
	multiplayer.multiplayer_peer = null
	_set_connection_state("disconnected")
	
	server_version = ""
	server_git_hash = ""

	
	# Attempt reconnection if enabled
	if Profile.auto_reconnect:
		_start_reconnect_timer()

# Helper function to check if we should auto-connect
func _check_auto_connect():
	if not Profile.auto_reconnect:
		print("[Networking] Auto-connect disabled in Profile")
		return

	# Check if running in a web browser
	if OS.has_feature("web"):
		var hostname = JavaScriptBridge.eval("window.location.hostname")
		print("Running on hostname: ", hostname)
		
		# Auto-connect if running on alpha.lunco.space
		if hostname == "alpha.lunco.space":
			print("Auto-connecting to server (running on alpha.lunco.space)")
			connect_to_server("langrenus.lunco.space", 9000, true)

# Helper function to setup reconnection timer
func _setup_reconnect_timer():
	reconnect_timer = Timer.new()
	reconnect_timer.name = "ReconnectTimer"
	reconnect_timer.one_shot = true
	reconnect_timer.timeout.connect(_attempt_reconnect)
	add_child(reconnect_timer)

# Helper function to start reconnection timer
func _start_reconnect_timer():
	if reconnect_timer:
		print("Will attempt reconnection in %.1f seconds..." % reconnect_delay)
		reconnect_timer.start(reconnect_delay)

# Helper function to attempt reconnection
func _attempt_reconnect():
	if last_connection_ip != "":
		print("Attempting to reconnect to %s:%d..." % [last_connection_ip, last_connection_port])
		connect_to_server(last_connection_ip, last_connection_port, last_connection_tls)
	else:
		print("No previous connection to reconnect to")

# Helper function to set connection state and emit signal
func _set_connection_state(new_state: String):
	if connection_state != new_state:
		connection_state = new_state
		print("Connection state changed: ", new_state)
		connection_state_changed.emit(new_state)
