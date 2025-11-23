extends Node

# TelemetryServer - Singleton that configures and starts the HTTP server for telemetry API

var http_server: HttpServer
var telemetry_router: TelemetryRouter

const TELEMETRY_PORT = 8082

func _ready():
	# Create HTTP server
	http_server = HttpServer.new()
	add_child(http_server)
	
	# Create and register telemetry router
	telemetry_router = TelemetryRouter.new()
	http_server.register_router("/api", telemetry_router)
	
	# Start server
	var err = http_server.start(TELEMETRY_PORT)
	if err == OK:
		print("Telemetry API server started on port %d" % TELEMETRY_PORT)
		print("Access telemetry at: http://localhost:%d/api/entities" % TELEMETRY_PORT)
	else:
		push_error("Failed to start telemetry server on port %d: %s" % [TELEMETRY_PORT, error_string(err)])

func _exit_tree():
	if http_server:
		http_server.stop()
