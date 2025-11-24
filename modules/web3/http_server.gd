class_name HttpServer
extends Node

# HTTP server properties
var server: TCPServer
var clients = []
var routers = {}
var default_router = null
var port = 8080

# TLS support
var tls_enabled = false
var tls_options: TLSOptions = null

func _init():
	server = TCPServer.new()

func _process(_delta):
	# Accept new connections
	if server.is_connection_available():
		var client = server.take_connection()
		
		# Wrap with TLS if enabled
		if tls_enabled and tls_options:
			var tls_client = StreamPeerTLS.new()
			var err = tls_client.accept_stream(client, tls_options)
			if err == OK:
				clients.append(tls_client)
			else:
				push_error("Failed to establish TLS connection: " + error_string(err))
		else:
			clients.append(client)
	
	# Process existing clients
	for i in range(clients.size() - 1, -1, -1):
		var client = clients[i]
		var status = StreamPeerTCP.STATUS_NONE
		
		# Get status based on client type
		if client is StreamPeerTLS:
			status = client.get_status()
			# For TLS, also check if handshake is complete
			if status == StreamPeerTLS.STATUS_CONNECTED:
				if client.get_available_bytes() > 0:
					_handle_client_request(client)
					clients.remove_at(i)
			elif status == StreamPeerTLS.STATUS_ERROR or status == StreamPeerTLS.STATUS_ERROR_HOSTNAME_MISMATCH:
				clients.remove_at(i)
		elif client is StreamPeerTCP:
			status = client.get_status()
			if status == StreamPeerTCP.STATUS_CONNECTED:
				if client.get_available_bytes() > 0:
					_handle_client_request(client)
					clients.remove_at(i)
			else:
				clients.remove_at(i)


func configure_tls(cert_path: String, key_path: String) -> Error:
	"""Configure TLS/SSL for HTTPS support"""
	# Load certificate
	var cert = X509Certificate.new()
	var cert_err = cert.load(cert_path)
	if cert_err != OK:
		push_error("Failed to load certificate from " + cert_path + ": " + error_string(cert_err))
		return cert_err
	
	# Load private key
	var key = CryptoKey.new()
	var key_err = key.load(key_path)
	if key_err != OK:
		push_error("Failed to load private key from " + key_path + ": " + error_string(key_err))
		return key_err
	
	# Create TLS options
	tls_options = TLSOptions.server(key, cert)
	tls_enabled = true
	
	print("TLS configured successfully")
	return OK

func start(p: int = 8080) -> Error:
	port = p
	var err = server.listen(port)
	if err != OK:
		push_error("Failed to start HTTP server on port " + str(port) + ": " + str(err))
	else:
		var protocol = "HTTPS" if tls_enabled else "HTTP"
		print(protocol + " server started on port " + str(port))
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