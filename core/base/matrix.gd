## This class is responsible for visualization of objects from Universe
## Particularly using Visual property of LCSPaceSystems
## It runs locally and adjusted visualization to each player according to their
## avatar position in the world
## It inplies that nodes in Universe should have no visual component
## For test it could be done by hiding them
@icon("res://core/base/matrix.svg")
@tool
class_name LCMatrix
extends LCSpaceSystem

#------------------

@export_category("Matrix Special")
@export var NodeToTrack: Node: set = set_tracking

#------------------

func _ready():
	var space_systems = []
	search_for_space_systems(NodeToTrack, space_systems)
	
	#visualising LCSpaceSystems
	for sp_system in space_systems:
		if sp_system.Visual:
			var n: Node3D = sp_system.Visual.instantiate()
			n.set_physics_process(false)
			n.global_position = sp_system.global_position
			add_child(n)

#------------------

func search_for_space_systems(node: Node, results: Array):
	if node is LCSpaceSystem:
		results.append(node)
	
	for children in node.get_children():
		search_for_space_systems(children, results)
#------------------

func set_tracking(node_to_track: Node):
	stop_tracking()
	NodeToTrack = node_to_track
	if NodeToTrack:
		NodeToTrack.child_entered_tree.connect(child_entered_tree)
		NodeToTrack.child_exiting_tree.connect(child_exiting_tree)

func stop_tracking():
	if NodeToTrack:
		NodeToTrack.child_entered_tree.disconnect(child_entered_tree)
		NodeToTrack.child_exiting_tree.disconnect(child_exiting_tree)
	
#------------------
func child_entered_tree(children):
	print("Entering: ", children)

func child_exiting_tree(children):
	pass
#func children_exi
