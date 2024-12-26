extends Node
class_name Utils

static var class_to_script_map = {}

static func initialize_class_map(path: String) -> void:
	var script_paths = get_script_paths(path)
	for script_path in script_paths:
		var script = load(script_path)
		var custom_class_name = get_custom_class_name_script(script)
		if custom_class_name:
			class_to_script_map[custom_class_name] = script_path

static func get_script_path(custom_class_name: String) -> String:
	return class_to_script_map.get(custom_class_name, "")

static func get_scene_paths(directory_path: String) -> Array:
	var dir = DirAccess.open(directory_path)
	print("get_scene: ", directory_path)
	
	var paths = []
	if dir:
		print(dir.get_files())
		dir.list_dir_begin()
		var file_name = dir.get_next()
		while file_name != "":
			if file_name.ends_with(".tscn"):
				paths.append(directory_path + file_name)
			elif file_name.ends_with(".tscn.remap"):
				paths.append(directory_path + file_name.left(-6))
			file_name = dir.get_next()
	return paths

static func get_script_paths(directory_path: String) -> Array:
	var dir = DirAccess.open(directory_path)
	
	var paths = []
	if dir:

		dir.list_dir_begin()
		var file_name = dir.get_next()
		while file_name != "":
			if file_name.ends_with(".gd"):
				paths.append(directory_path + file_name)
			file_name = dir.get_next()
	return paths

static func get_custom_class_name(node: Node) -> String:
	var script = node.get_script()

	var custom_class_name = get_custom_class_name_script(script)
	if custom_class_name == "":
		custom_class_name = node.get_class()

	return custom_class_name

static func get_custom_class_name_script(script: Script) -> String:
	if script:
		return script.get_path().get_file().get_basename()
	return ""
