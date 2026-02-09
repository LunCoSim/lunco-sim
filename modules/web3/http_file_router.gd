class_name HttpFileRouter
extends HttpRouter

var root_path: String
var default_file: String = "index.html"
var mime_types: Dictionary = {
	"html": "text/html",
	"htm": "text/html",
	"css": "text/css",
	"js": "application/javascript",
	"json": "application/json",
	"png": "image/png",
	"jpg": "image/jpeg",
	"jpeg": "image/jpeg",
	"gif": "image/gif",
	"svg": "image/svg+xml",
	"ico": "image/x-icon",
	"txt": "text/plain",
	"pdf": "application/pdf",
	"zip": "application/zip"
}

func _init(path: String = "res://"):
	root_path = path
	if not root_path.ends_with("/"):
		root_path += "/"

func handle_get(request, response) -> void:
	var file_path = request.path
	
	# Remove any query parameters
	if file_path.find("?") >= 0:
		file_path = file_path.split("?")[0]
	
	# Handle directory requests
	if file_path.ends_with("/"):
		file_path += default_file
	
	# If path is empty or root, serve the default file
	if file_path == "" or file_path == "/":
		file_path = "/" + default_file
	
	# Remove leading slash for relative paths
	if file_path.begins_with("/"):
		file_path = file_path.substr(1)
	
	# Create the full path
	var full_path = root_path + file_path
	
	# Check if file exists
	if FileAccess.file_exists(full_path):
		_serve_file(full_path, response)
	else:
		response.send_error(404, "File Not Found")

func _serve_file(file_path: String, response) -> void:
	response.send_file(file_path) 