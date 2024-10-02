@tool
# Blank facility, just for testing. Visualised as a simple box.
class_name LCFacilityBlank
extends LCFacility

@export var size: Vector3 = Vector3(10, 5, 10)
@export var color: Color = Color.GRAY

var mesh_instance: MeshInstance3D

func _ready():
	create_visual()

func create_visual():
	mesh_instance = MeshInstance3D.new()
	var box_mesh = BoxMesh.new()
	box_mesh.size = size
	mesh_instance.mesh = box_mesh
	
	var material = StandardMaterial3D.new()
	material.albedo_color = color
	mesh_instance.set_surface_override_material(0, material)
	
	add_child(mesh_instance)

func set_size(new_size: Vector3):
	size = new_size
	if mesh_instance:
		mesh_instance.mesh.size = size

func set_color(new_color: Color):
	color = new_color
	if mesh_instance and mesh_instance.get_surface_override_material(0):
		mesh_instance.get_surface_override_material(0).albedo_color = color
