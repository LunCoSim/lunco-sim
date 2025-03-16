class_name HttpRouter
extends RefCounted

# Basic router implementation for HTTP server

func handle_request(request, response) -> void:
	match request.method:
		"GET":
			handle_get(request, response)
		"POST":
			handle_post(request, response)
		"PUT":
			handle_put(request, response)
		"DELETE":
			handle_delete(request, response)
		"OPTIONS":
			handle_options(request, response)
		_:
			response.set_status(405, "Method Not Allowed")
			response.send("Method not allowed", "text/plain")

# Default handlers for different HTTP methods
func handle_get(request, response) -> void:
	response.send_error(404, "Not Found")

func handle_post(request, response) -> void:
	response.send_error(404, "Not Found")

func handle_put(request, response) -> void:
	response.send_error(404, "Not Found")

func handle_delete(request, response) -> void:
	response.send_error(404, "Not Found")

func handle_options(request, response) -> void:
	response.set_header("Allow", "GET, POST, PUT, DELETE, OPTIONS")
	response.send("", "text/plain") 