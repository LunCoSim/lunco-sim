class_name LCChassisEffector
extends LCStateEffector

## Parametric chassis component.
## Allows configuring size and automatically updates mass and collision.

@export var size: Vector3 = Vector3(1.0, 0.5, 2.0):
	set(value):
		size = value
		_update_dimensions()

@export var density: float = 50.0: # kg/m^3
	set(value):
		density = value
		_update_mass()

@onready var mesh_instance: MeshInstance3D = $MeshInstance3D
@onready var collision_shape: CollisionShape3D = $CollisionShape3D
# Attachment points...
@onready var attach_top: Marker3D = $AttachTop
@onready var attach_bottom: Marker3D = $AttachBottom
@onready var attach_front: Marker3D = $AttachFront
@onready var attach_back: Marker3D = $AttachBack
@onready var attach_left: Marker3D = $AttachLeft
@onready var attach_right: Marker3D = $AttachRight

func _ready():
	super._ready()
	_update_dimensions()

func _update_dimensions():
	if not is_inside_tree(): return
	
	# Update Mesh
	if mesh_instance:
		if mesh_instance.mesh is BoxMesh:
			mesh_instance.mesh.size = size
		elif not mesh_instance.mesh:
			var m = BoxMesh.new()
			m.size = size
			mesh_instance.mesh = m
		
	# Update Collision
	if collision_shape:
		if collision_shape.shape is BoxShape3D:
			collision_shape.shape.size = size
		elif not collision_shape.shape:
			var s = BoxShape3D.new()
			s.size = size
			collision_shape.shape = s
	
	# Update Attachment Points (simplified for brevity, can add back if needed)
	if attach_top: attach_top.position = Vector3(0, size.y/2, 0)
	if attach_bottom: attach_bottom.position = Vector3(0, -size.y/2, 0)
	if attach_front: attach_front.position = Vector3(0, 0, -size.z/2)
	if attach_back: attach_back.position = Vector3(0, 0, size.z/2)
	if attach_left: attach_left.position = Vector3(-size.x/2, 0, 0)
	if attach_right: attach_right.position = Vector3(size.x/2, 0, 0)
	
	_update_mass()

func _update_mass():
	# Volume * Density
	var volume = size.x * size.y * size.z
	mass = volume * density
	# Notify parent if possible (in editor)
	update_configuration_warnings()

