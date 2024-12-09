extends Node2D

@onready var graph_edit: GraphEdit = $GraphEdit

func _ready():
	# Connect signals for handling connections
	graph_edit.connect("connection_request", _on_connection_request)
	graph_edit.connect("disconnection_request", _on_disconnection_request)
	
	# Enable snapping and minimap for better UX
	graph_edit.snapping_distance = 20
	#graph_edit.show_minimap = true
	#graph_edit.minimap_enabled = true

func _on_connection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int):
	# Create new connection between nodes
	graph_edit.connect_node(from_node, from_port, to_node, to_port)
	
	# You can add custom logic here for handling the resource flow
	print("Connected: ", from_node, "(", from_port, ") -> ", to_node, "(", to_port, ")")

func _on_disconnection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int):
	# Remove connection between nodes
	graph_edit.disconnect_node(from_node, from_port, to_node, to_port)
	
	# You can add custom cleanup logic here
	print("Disconnected: ", from_node, "(", from_port, ") -> ", to_node, "(", to_port, ")")
