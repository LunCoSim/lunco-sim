extends SceneTree

func _init():
	print("Checking GraphNode signals in Godot 4.4.rc1:")
	var node = GraphNode.new()
	for signal_info in node.get_signal_list():
		print("Signal: " + signal_info.name)
	quit() 
