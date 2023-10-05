@icon("res://controllers/player/character.svg")
class_name LCPlayer
extends LCSpaceSystem


func set_camera(camera):
#	print("lnPlayer set_camera")
	if %InputSynchronizer != null:
		%InputSynchronizer.set_camera(camera)

#TBD: Implement
func remove_camera(camera):
	pass
	
	#if %InputSynchronizer != null:
		#%InputSynchronizer.remove_c
