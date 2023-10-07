# This class is a tool node extending 'Node'. It has capabilities of finding and grabbing 3D cameras from the scene.
# It has two main methods:
# 1. find_camera: which recursively searches the scene graph (excluding a particular node) to find all 'Camera3D' instances and appends them into an array.
# 2. grab_camera: which retrieves the first available 'Camera3D' instance from the editor's main screen (in editor mode) or from the 'Viewport' (in game mode).

@tool
extends Node

# Function to find a Camera3D node from a list, excluding a specific node
static func find_camera(from: Array, exclude, cameras: Array):
	for node in from:
		# Avoid searching the node that is meant to be excluded
		if node != exclude:
			# Recursive search
			find_camera(node.get_children(), exclude, cameras)
		# Append the node to the 'cameras' list if it's a 'Camera3D' instance
		if node is Camera3D:
			cameras.append(node)

# Function to grab a Camera3D node
func grab_camera() -> Camera3D:
	var _camera: Camera3D

	# Check if the game is running in the editor
	if Engine.is_editor_hint():
		# List to store the found 'Camera3D' nodes
		var cameras = []

		# Find 'Camera3D' nodes in the editor's main screen
		find_camera(EditorInterface.get_editor_main_screen().get_children(), EditorInterface.get_edited_scene_root(), cameras)

		# Use the first found 'Camera3D' node
		if cameras.size():
			_camera = cameras[0]

	# Check if the game is running outside of the editor
	else:
		# Get the current 'Camera3D' node from the 'Viewport' node
		if get_viewport():
			_camera = get_viewport().get_camera_3d()
		
	# Return the grabbed 'Camera3D' node
	return _camera
