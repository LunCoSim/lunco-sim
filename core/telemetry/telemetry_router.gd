class_name TelemetryRouter
extends HttpRouter

# Router for telemetry API endpoints

func handle_get(request: HttpRequest, response: HttpResponse) -> void:
	# Add CORS headers
	response.set_header("Access-Control-Allow-Origin", "*")
	response.set_header("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS")
	response.set_header("Access-Control-Allow-Headers", "Content-Type")
	
	var path = request.path
	
	# Remove /api prefix if present
	if path.begins_with("/api"):
		path = path.substr(4)
	
	# Route to appropriate handler
	if path == "/entities":
		_handle_entities(request, response)
	elif path.begins_with("/telemetry/"):
		_handle_telemetry(request, response)
	elif path == "/dictionary":
		_handle_dictionary(request, response)
	elif path == "/events":
		_handle_global_events(request, response)
	elif path.begins_with("/events/"):
		_handle_entity_events(request, response)
	elif path == "/command":
		_handle_command_definitions(request, response)
	elif path == "/images":
		_handle_image_list(request, response)
	elif path.begins_with("/images/"):
		_handle_image_request(request, response)
	else:
		response.send_error(404, "Not Found")

func _handle_image_list(_request: HttpRequest, response: HttpResponse) -> void:
	var images = TelemetryManager.get_image_list()
	response.send_json({"images": images})

func handle_post(request: HttpRequest, response: HttpResponse) -> void:
	# Add CORS headers
	response.set_header("Access-Control-Allow-Origin", "*")
	response.set_header("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS")
	response.set_header("Access-Control-Allow-Headers", "Content-Type")
	
	var path = request.path
	if path.begins_with("/api"):
		path = path.substr(4)
		
	if path == "/command":
		_handle_command(request, response)
	else:
		response.send_error(404, "Not Found")

func handle_delete(request: HttpRequest, response: HttpResponse) -> void:
	# Add CORS headers
	response.set_header("Access-Control-Allow-Origin", "*")
	response.set_header("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS")
	response.set_header("Access-Control-Allow-Headers", "Content-Type")
	
	var path = request.path
	if path.begins_with("/api"):
		path = path.substr(4)
		
	if path.begins_with("/entities/"):
		_handle_delete_entity(request, response)
	else:
		response.send_error(404, "Not Found")

func handle_options(request: HttpRequest, response: HttpResponse) -> void:
	# Handle CORS preflight
	response.set_header("Access-Control-Allow-Origin", "*")
	response.set_header("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS")
	response.set_header("Access-Control-Allow-Headers", "Content-Type")
	response.send("", "text/plain")

func _handle_entities(_request: HttpRequest, response: HttpResponse) -> void:
	var entities = TelemetryManager.get_entities()
	response.send_json({"entities": entities})

func _handle_telemetry(request: HttpRequest, response: HttpResponse) -> void:
	# Extract entity_id from path: /telemetry/{entity_id} or /telemetry/{entity_id}/history
	var path = request.path
	if path.begins_with("/api"):
		path = path.substr(4)
	
	var parts = path.split("/")
	if parts.size() < 3:
		response.send_error(400, "Invalid telemetry path")
		return
	
	var entity_id = parts[2]
	
	# Check if this is a history request
	if parts.size() > 3 and parts[3] == "history":
		_handle_history(entity_id, request, response)
	else:
		_handle_latest(entity_id, response)

func _handle_latest(entity_id: String, response: HttpResponse) -> void:
	var data = TelemetryManager.get_latest_telemetry(entity_id)
	if data.is_empty():
		response.send_error(404, "Entity not found")
	else:
		# print("DEBUG: Sending latest telemetry for ", entity_id, ": ", data.keys())
		response.send_json(data)

func _handle_history(entity_id: String, request: HttpRequest, response: HttpResponse) -> void:
	# Parse query parameters for time range
	var start_time = int(request.get_parameter("start", "0"))
	var end_time = int(request.get_parameter("end", "0"))
	
	var history = TelemetryManager.get_history(entity_id, start_time, end_time)
	response.send_json({"history": history})

func _handle_dictionary(_request: HttpRequest, response: HttpResponse) -> void:
	var dictionary = TelemetryManager.get_openmct_dictionary()
	response.send_json(dictionary)

func _handle_global_events(request: HttpRequest, response: HttpResponse) -> void:
	# Parse query parameters for time range
	var start_time = int(request.get_parameter("start", "0"))
	var end_time = int(request.get_parameter("end", "0"))
	
	var events = TelemetryManager.get_global_events(start_time, end_time)
	response.send_json({"events": events})

func _handle_entity_events(request: HttpRequest, response: HttpResponse) -> void:
	# Extract entity_id from path: /events/{entity_id}
	var path = request.path
	if path.begins_with("/api"):
		path = path.substr(4)
	
	var parts = path.split("/")
	if parts.size() < 3:
		response.send_error(400, "Invalid events path")
		return
	
	var entity_id = parts[2]
	
	# Parse query parameters for time range
	var start_time = int(request.get_parameter("start", "0"))
	var end_time = int(request.get_parameter("end", "0"))
	
	var events = TelemetryManager.get_entity_events(entity_id, start_time, end_time)
	response.send_json({"events": events})

func _handle_command(request: HttpRequest, response: HttpResponse) -> void:
	var body_str = request.body
	var json = JSON.new()
	var err = json.parse(body_str)
	
	if err != OK:
		response.send_error(400, "Invalid JSON: " + json.get_error_message())
		return
		
	var data = json.get_data()
	if not data is Dictionary:
		response.send_error(400, "Expected JSON object")
		return
		
	# Dispatch command via LCCommandRouter
	# Remote commands should be marked as source="http"
	data["source"] = "http"
	var result = await LCCommandRouter.execute_raw(data)
	
	if result is String and result.begins_with("Command target not found"):
		response.send_error(404, result)
	elif result is String and result.begins_with("Parent"):
		response.send_error(400, result)
	else:
		response.send_json({"status": "executed", "result": result})

func _handle_command_definitions(_request: HttpRequest, response: HttpResponse) -> void:
	var defs = LCCommandRouter.get_all_command_definitions()
	response.send_json({"targets": defs})

func _handle_delete_entity(request: HttpRequest, response: HttpResponse) -> void:
	# Extract entity_id from path: /entities/{entity_id}
	var path = request.path
	if path.begins_with("/api"):
		path = path.substr(4)
	
	var parts = path.split("/")
	if parts.size() < 3:
		response.send_error(400, "Invalid entity path")
		return
	
	var entity_id = parts[2]
	
	# Execute DELETE command via LCCommandRouter
	var command_data = {
		"target_path": "Simulation",
		"name": "DELETE",
		"arguments": {"entity_id": entity_id},
		"source": "http"
	}
	
	var result = await LCCommandRouter.execute_raw(command_data)
	
	if result is String and result.begins_with("Entity not found"):
		response.send_error(404, result)
	elif result is String and result.begins_with("Cannot delete entity"):
		# Entity is currently being controlled
		response.send_error(409, result)  # 409 Conflict
	else:
		response.send_json({"status": "deleted", "result": result})

func _handle_image_request(request: HttpRequest, response: HttpResponse) -> void:
	var path = request.path
	if path.begins_with("/api"):
		path = path.substr(4)
	
	var parts = path.split("/")
	if parts.size() < 3:
		response.send_error(400, "Invalid image path")
		return
	
	var filename = parts[2]
	var file_path = "user://images/" + filename
	
	if not FileAccess.file_exists(file_path):
		response.send_error(404, "Image not found")
		return
	
	response.send_file(file_path)
