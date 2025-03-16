class_name HttpRequest
extends RefCounted

var method: String = ""
var path: String = ""
var http_version: String = ""
var headers: Dictionary = {}
var body: String = ""
var query_params: Dictionary = {}
var body_parsed: Dictionary = {}

func parse(request_data: String) -> void:
	var lines = request_data.split("\r\n")
	if lines.size() < 1:
		return
	
	# Parse request line (method, path, version)
	var request_line = lines[0].split(" ")
	if request_line.size() >= 3:
		method = request_line[0]
		path = request_line[1]
		http_version = request_line[2]
	
	# Parse headers
	var i = 1
	while i < lines.size():
		var line = lines[i]
		if line.is_empty():
			i += 1
			break
		
		var header_parts = line.split(":", true, 1)
		if header_parts.size() == 2:
			headers[header_parts[0].strip_edges()] = header_parts[1].strip_edges()
		i += 1
	
	# Parse body
	if i < lines.size():
		body = "\r\n".join(lines.slice(i))
		_parse_body()
	
	# Parse query parameters
	_parse_query_parameters()

func _parse_query_parameters() -> void:
	if path.find("?") != -1:
		var path_parts = path.split("?", true, 1)
		path = path_parts[0]
		
		if path_parts.size() > 1:
			var query = path_parts[1]
			var params = query.split("&")
			
			for param in params:
				var key_value = param.split("=", true, 1)
				if key_value.size() == 2:
					query_params[key_value[0]] = key_value[1]

func _parse_body() -> void:
	if method == "POST":
		var content_type = headers.get("Content-Type", "")
		
		if content_type.begins_with("application/json"):
			var json = JSON.new()
			var parse_result = json.parse(body)
			if parse_result == OK:
				body_parsed = json.data
		elif content_type.begins_with("application/x-www-form-urlencoded"):
			var params = body.split("&")
			for param in params:
				var key_value = param.split("=", true, 1)
				if key_value.size() == 2:
					body_parsed[key_value[0]] = key_value[1].uri_decode()

func get_header(key: String, default: String = "") -> String:
	return headers.get(key, default)

func get_parameter(key: String, default: String = "") -> String:
	return query_params.get(key, default)

func get_body_parsed() -> Dictionary:
	return body_parsed 