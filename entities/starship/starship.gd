@icon("res://entities/starship_entity.svg")
extends LCRigidBody


# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(_delta):
	pass


func _on_spacecraft_controller_thrusted(enabled):
	$RocketEngine.visible = enabled
			
