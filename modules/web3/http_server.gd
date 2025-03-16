class_name HttpServer
extends Node

# HTTP server properties
var server: TCPServer
var clients = []
var routers = {}
var default_router = null
var port = 8080

func _init():
	server = TCPServer.new()

func _process(_delta):
	# Accept new connections
	if server.is_connection_available():
		var client = server.take_connection()
		clients.append(client)
	
	# Process existing clients
	for i in range(clients.size() - 1, -1, -1):
		var client = clients[i]
		if client.get_status() == StreamPeerTCP.STATUS_CONNECTED:
			if client.get_available_bytes() > 0:
				_handle_client_request(client)
				clients.remove_at(i)
		else:
			clients.remove_at(i)

func start(p: int = 8080) -> Error:
	port = p
	var err = server.listen(port)
	if err != OK:
		push_error("Failed to start HTTP server on port " + str(port) + ": " + str(err))
	else:
		print("HTTP server started on port " + str(port))
	return err

func stop() -> void:
	server.stop()
	print("HTTP server stopped")

func register_router(path: String, router) -> void:
	routers[path] = router
	
func set_default_router(router) -> void:
	default_router = router

func _handle_client_request(client: StreamPeerTCP) -> void:
	# Read request data
	var request_data = client.get_utf8_string(client.get_available_bytes())
	
	# Parse request
	var request = HttpRequest.new()
	request.parse(request_data)
	
	# Create response object
	var response = HttpResponse.new(client)
	
	# Find the correct router
	var router = null
	var best_match_length = -1
	
	for path in routers:
		if request.path.begins_with(path) and path.length() > best_match_length:
			router = routers[path]
			best_match_length = path.length()
	
	# If no router found, use default router
	if router == null:
		router = default_router
	
	# Handle request
	if router != null:
		router.handle_request(request, response)
	else:
		response.send_error(404, "Not Found") 