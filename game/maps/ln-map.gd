class_name LnMap
extends Spatial

signal clicked(position)

func _on_Surface_input_event(camera, event, position, normal, shape_idx):
	if event is InputEventMouseButton:
		emit_signal("clicked", position)

func _on_EmptyMap_clicked(position):
	print("_on_EmpteMap_clicked: ", position)


func _on_Terrain_clicked(position):
	print("_on_Terrain_clicked: ", position)
