extends Node

# Launches the dedicated signaling server for WebRTC
func _ready():
	var server = HttpServer.new()
	server.register_router("/signal", SignalingServer.new())
	add_child(server)
	server.start(8081) # Use a separate port for signaling, e.g., 8081
