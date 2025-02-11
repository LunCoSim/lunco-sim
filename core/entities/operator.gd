@icon("res://core/entities/operator-entity.svg")
extends CharacterBody3D

func _ready():
	var material: StandardMaterial3D = $MeshInstance.get_surface_override_material(0)
	material = material.duplicate()
	
	var idx = str(self.name).to_int()
	
	material.albedo_color = Color(sin(idx)**2, cos(idx)**2, (sin(idx)-cos(idx))**2)
	
	$MeshInstance.set_surface_override_material(0, material)
