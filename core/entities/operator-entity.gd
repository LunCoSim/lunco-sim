extends CharacterBody3D

func _ready():
	var material = $MeshInstance.get_surface_override_material(0)
	
	var idx = self.get_parent().get_child_count()
	
	material.albedo_color = Color(sin(idx)**2, cos(idx)**2, (sin(idx)-cos(idx))**2)
