extends Node

# TelemetryServer - Singleton that configures and starts the HTTP server for telemetry API

var http_server: HttpServer
var telemetry_router: TelemetryRouter

const TELEMETRY_PORT = 8082

# Start the telemetry server with optional TLS
# If cert and key paths are provided, HTTPS will be enabled
# Otherwise, HTTP will be used (suitable for local development)
func start_server(tls_cert_path: String = "", tls_key_path: String = ""):
	# Create HTTP server
	http_server = HttpServer.new()
	add_child(http_server)
	
	# Configure TLS if paths are provided
	if tls_cert_path and tls_key_path:
		print("Setting up HTTPS with cert: %s, key: %s" % [tls_cert_path, tls_key_path])
		var tls_err = http_server.configure_tls(tls_cert_path, tls_key_path)
		if tls_err == OK:
			print("HTTPS enabled for telemetry server")
		else:
			push_warning("Failed to configure TLS, falling back to HTTP")
	else:
		print("No TLS certificates provided, using HTTP")
	
	# Create and register telemetry router
	telemetry_router = TelemetryRouter.new()
	http_server.register_router("/api", telemetry_router)
	
	# Start server
	var err = http_server.start(TELEMETRY_PORT)
	if err == OK:
		var protocol = "HTTPS" if http_server.tls_enabled else "HTTP"
		print("Telemetry API server started on port %d (%s)" % [TELEMETRY_PORT, protocol])
		print("Access telemetry at: %s://localhost:%d/api/entities" % [protocol.to_lower(), TELEMETRY_PORT])
	else:
		push_error("Failed to start telemetry server on port %d: %s" % [TELEMETRY_PORT, error_string(err)])

func _ready():
	# Parse command line arguments for TLS certificate paths
	var arguments = OS.get_cmdline_args()
	var certificate_path = ""
	var key_path = ""
	
	# Check for --certificate and --key arguments
	for i in range(arguments.size()):
		match arguments[i]:
			"--certificate":
				if i + 1 < arguments.size():
					certificate_path = arguments[i + 1]
			"--key":
				if i + 1 < arguments.size():
					key_path = arguments[i + 1]
	
	# Start server with TLS if both certificate and key are provided
	var use_tls = certificate_path != "" and key_path != ""
	
	if use_tls:
		print("Starting telemetry server with HTTPS")
		print("Certificate: ", certificate_path)
		print("Private Key: ", key_path)
		start_server(certificate_path, key_path)
	else:
		print("Starting telemetry server with HTTP (no TLS certificates provided)")
		start_server()

func _exit_tree():
	if http_server:
		http_server.stop()
