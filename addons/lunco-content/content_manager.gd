@tool
extends EditorPlugin

const ContentFile = preload("res://addons/lunco-content/content_file.gd")

var _download_queue = []
var _current_download: HTTPRequest
var _menu_button: MenuButton

func _enter_tree() -> void:
    _menu_button = MenuButton.new()
    _menu_button.text = "Content"
    var popup = _menu_button.get_popup()
    popup.add_item("Check Missing Content", 0)
    popup.add_item("Download All Content", 1)
    popup.id_pressed.connect(_on_menu_item_pressed)
    
    add_control_to_container(EditorPlugin.CONTAINER_TOOLBAR, _menu_button)
    
    # Automatically check for missing content on startup
    call_deferred("check_and_download_content")

func _exit_tree() -> void:
    if _menu_button:
        remove_control_from_container(EditorPlugin.CONTAINER_TOOLBAR, _menu_button)
        _menu_button.queue_free()

func _on_menu_item_pressed(id: int) -> void:
    match id:
        0: check_content()
        1: download_all_content()

func check_and_download_content() -> void:
    var missing = find_missing_content()
    if not missing.is_empty():
        print("Found missing content files: ", missing.size())
        download_all_content()

func check_content() -> void:
    var missing = find_missing_content()
    if missing.is_empty():
        print("All content files present")
    else:
        print("Missing files: ", missing)
        for file in missing:
            print("- ", file["path"])

func download_all_content() -> void:
    var missing = find_missing_content()
    if missing.is_empty():
        print("No files to download")
        return
        
    _download_queue = missing.duplicate()
    print("Starting download of ", _download_queue.size(), " files...")
    _process_download_queue()

func find_missing_content() -> Array:
    var missing = []
    var dir = DirAccess.open("res://")
    if dir:
        _scan_dir(dir, missing)
    return missing

func _scan_dir(dir: DirAccess, missing: Array) -> void:
    dir.list_dir_begin()
    var file_name = dir.get_next()
    
    while file_name != "":
        var current_dir = dir.get_current_dir()
        var full_path = current_dir.path_join(file_name)
        
        if dir.current_is_dir() and not file_name.begins_with("."):
            var subdir = DirAccess.open(full_path)
            if subdir:
                _scan_dir(subdir, missing)
        elif file_name.ends_with(".content"):
            var content_file = ContentFile.new()
            var err = content_file.load(full_path)
            if err == OK:
                var target_file = full_path.trim_suffix(".content")
                if not FileAccess.file_exists(target_file):
                    missing.append({"path": target_file, "content": content_file})
            else:
                push_error("Failed to load content file: " + full_path)
                
        file_name = dir.get_next()
    
    dir.list_dir_end()

func _process_download_queue() -> void:
    if _download_queue.is_empty():
        print("All downloads completed")
        return
        
    if _current_download != null:
        return
        
    var next_file = _download_queue[0]
    var content_file = next_file["content"]
    
    print("Downloading: ", next_file["path"])
    
    _current_download = HTTPRequest.new()
    add_child(_current_download)
    _current_download.request_completed.connect(_on_download_completed.bind(next_file))
    
    # Set longer timeout for large files
    _current_download.timeout = 3600  # 1 hour timeout
    
    var headers = ["Accept: */*"]
    var error = _current_download.request(content_file.url, headers)
    if error != OK:
        push_error("Failed to start download: " + next_file["path"] + " (Error: " + str(error) + ")")
        _cleanup_current_download()
        _download_queue.pop_front()
        _process_download_queue()

func _on_download_completed(result: int, response_code: int, _headers: PackedStringArray, body: PackedByteArray, file_info: Dictionary) -> void:
    if result != HTTPRequest.RESULT_SUCCESS:
        push_error("Download failed: " + file_info["path"] + " (Result: " + str(result) + ", Code: " + str(response_code) + ")")
    elif response_code != 200:
        push_error("HTTP Error: " + file_info["path"] + " (Code: " + str(response_code) + ")")
    else:
        # Ensure the directory exists
        var dir = DirAccess.open("res://")
        if dir:
            var path = file_info["path"]
            var dir_path = path.get_base_dir()
            if not dir.dir_exists(dir_path):
                dir.make_dir_recursive(dir_path)
        
        var file = FileAccess.open(file_info["path"], FileAccess.WRITE)
        if file:
            file.store_buffer(body)
            print("Downloaded: " + file_info["path"])
        else:
            push_error("Failed to write file: " + file_info["path"])
    
    _cleanup_current_download()
    _download_queue.pop_front()
    _process_download_queue()

func _cleanup_current_download() -> void:
    if _current_download:
        _current_download.queue_free()
        _current_download = null 