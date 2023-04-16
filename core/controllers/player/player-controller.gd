extends lnSpaceSystem
class_name lnPlayer

func set_camera(camera):
	print("lnPlayer set_camera")
	%InputSynchronizer.set_camera(camera)
	
#	if $InputSynchronizer:
#		$InputSynchronizer.set_camera(camera)
