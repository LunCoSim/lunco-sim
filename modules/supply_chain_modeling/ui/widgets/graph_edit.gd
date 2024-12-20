extends GraphEdit

func clear_graph():
	for node in get_children():
		if node is GraphNode:
			node.free()
