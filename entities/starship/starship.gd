@icon("res://entities/starship_entity.svg")
extends LCRigidBody

func _on_spacecraft_controller_thrusted(enabled):
	$RocketEngine.visible = enabled
			
