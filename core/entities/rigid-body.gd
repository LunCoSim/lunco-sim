class_name LCRigidBody
extends RigidBody3D

func _ready():
	pass
 
func _on_spacecraft_controller_thrusted(thrust):
	$RocketEngine.visible = thrust
