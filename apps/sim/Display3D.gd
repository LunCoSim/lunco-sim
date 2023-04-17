extends MeshInstance3D


# Called when the node enters the scene tree for the first time.
func _ready():
	var viewport = $SubViewport
	$SubViewport.set_clear_mode(SubViewport.CLEAR_MODE_ONCE)
	
	
	# Retrieve the texture and set it to the viewport quad.
	material_override.albedo_texture = viewport.get_texture()



func _on_button_pressed():
	print("3D Display button pressed")
	pass # Replace with function body.
