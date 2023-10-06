@icon("res://core/base/map.svg")
@tool
class_name LCMap
extends LCSpaceSystem 

# Location of the map on the celestial body
@export var LocationCoordinates: Vector2

var camera: Camera3D

func _init():
	camera = LCUtil.grab_camera()
	
func _process(delta):
	
	#hiding/showing depending on distance. Overkill with LOD system, but let it
	if camera:
		var d = camera.position.distance_squared_to(position)
		
		if camera.position.distance_squared_to(position) > 1000*1000:
			self.hide()
		else:
			self.show()
