extends Spatial


var mouse_sens = 0.3
var camera_anglev=0

# func _input(event):         
# 	if event is InputEventMouseMotion:
# 		$Camera.rotate_y(deg2rad(-event.relative.x*mouse_sens))
# 		var changev=-event.relative.y*mouse_sens
# 		if camera_anglev+changev>-50 and camera_anglev+changev<50:
# 			camera_anglev+=changev
# 			$Camera.rotate_x(deg2rad(changev))
