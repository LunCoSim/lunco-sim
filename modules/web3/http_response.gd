class_name HttpResponse
extends RefCounted

var client  # Can be StreamPeerTCP or StreamPeerTLS
var headers: Dictionary = {}
var status_code: int = 200
var status_message: String = "OK"
var content_type: String = "text/html"
var body: String = ""

func _init(client_connection):  # Can be StreamPeerTCP or StreamPeerTLS
	client = client_connection
	
	# Set default headers
	headers["Server"] = "GodotHTTPServer/1.0"
	headers["Connection"] = "close"

func set_header(key: String, value: String) -> void:
	headers[key] = value

func set_content_type(type: String) -> void:
	content_type = type
	headers["Content-Type"] = type

func set_status(code: int, message: String = "") -> void:
	status_code = code
	if message.is_empty():
		match code:
			200: status_message = "OK"
			201: status_message = "Created"
			204: status_message = "No Content"
			400: status_message = "Bad Request"
			401: status_message = "Unauthorized"
			403: status_message = "Forbidden"
			404: status_message = "Not Found"
			500: status_message = "Internal Server Error"
			_: status_message = "Unknown"
	else:
		status_message = message

func send(content: String, type: String = "text/html") -> void:
	body = content
	set_content_type(type)
	headers["Content-Length"] = str(content.length())
	_send_response()

func send_json(data) -> void:
	var json_str = JSON.stringify(data)
	send(json_str, "application/json")

func send_file(file_path: String) -> void:
	if not FileAccess.file_exists(file_path):
		send_error(404, "File Not Found")
		return
	
	var file = FileAccess.open(file_path, FileAccess.READ)
	if not file:
		send_error(500, "Could not open file")
		return
	
	var content = file.get_as_text()
	file.close()
	
	# Determine content type based on file extension
	var extension = file_path.get_extension().to_lower()
	var type = "application/octet-stream" # Default
	
	match extension:
		"html", "htm": type = "text/html"
		"css": type = "text/css"
		"js": type = "application/javascript"
		"json": type = "application/json"
		"png": type = "image/png"
		"jpg", "jpeg": type = "image/jpeg"
		"gif": type = "image/gif"
		"svg": type = "image/svg+xml"
		"txt": type = "text/plain"
	
	send(content, type)

func send_error(code: int, message: String = "") -> void:
	set_status(code, message)
	var error_page = "<html><head><title>Error " + str(code) + "</title></head>"
	error_page += "<body><h1>Error " + str(code) + ": " + status_message + "</h1></body></html>"
	send(error_page, "text/html")

func redirect(url: String, permanent: bool = false) -> void:
	set_status(301 if permanent else 302)
	set_header("Location", url)
	send("", "text/plain")

func _send_response() -> void:
	var response = "HTTP/1.1 " + str(status_code) + " " + status_message + "\r\n"
	
	# Add headers
	for key in headers:
		response += key + ": " + headers[key] + "\r\n"
	
	# Add body
	response += "\r\n" + body
	
	# Send response
	client.put_data(response.to_utf8_buffer()) 