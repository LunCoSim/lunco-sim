@tool
extends Node


static func find_camera(from: Array, exclude, cameras: Array):
	for node in from:
		if node != exclude:
			find_camera(node.get_children(), exclude, cameras)
		if node is Camera3D:
			cameras.append(node)
	
func grab_camera() -> Camera3D:
	var _camera: Camera3D
#	_camera = get_viewport().get_camera_3d()
	if Engine.is_editor_hint():
		var a = EditorScript.new()
		var i = a.get_editor_interface()

		var cameras = []

		find_camera(i.get_editor_main_screen().get_children(), i.get_edited_scene_root(), cameras)

		print(":Grab camera: ", cameras)
		if cameras.size():
			_camera = cameras[0]
	else:
		_camera = get_viewport().get_camera_3d()
	
	return _camera
	
