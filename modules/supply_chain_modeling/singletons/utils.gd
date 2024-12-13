extends Node
class_name Utils

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
