@tool
extends MeshInstance3D

@export var Subdivisions: int = 2: set = _update_subdivsions
@export var Radius: float = 1.0: set = _update_radius
@export var Strength: float = 1.0: set = _update_strenth

# 6 planes: X+, X-, Y+, Y-, Z+, Z-

## Several level of detailse
## Height map: 23040x11520 pix (1012.6 MB) 
## 64 ppd (pixel per degree) 
## floating-point TIFFs in kilometers, 
## relative to a radius of 1737.4 km

var width = 23040
var height = 11520
var ppd = 64 #pixels per degree
var radius = 1737400 #meters
var rescale = Radius / radius

var planes = []

#------------------------------------------
func _ready():
	print("Rescale: ", rescale)
	_update_mesh()

func _update_subdivsions(value):
	Subdivisions = value
	_update_mesh()
	
func _update_radius(value):
	Radius = value
	_update_mesh()

func _update_strenth(value):
	Strength = value
	_update_mesh()

#------------------------------------------	
func _update_mesh():
	mesh = create_quad_sphere(Subdivisions, Radius)
	
	var material = StandardMaterial3D.new()
	material.vertex_color_use_as_albedo = true
	
	for i in range(mesh.get_surface_count()):
		mesh.surface_set_material(i, material)

#------------------------------------------
func create_plane(plane_normal:Vector3, size:=1.0):
	var half = size/2.0
	
	var plane = ArrayMesh.new()
	# X+ (1, 0, 0) -> 
	
	var lt = Vector3(half, half, -half)
	var rt = Vector3(half, half, half)
	var lb = Vector3(half, -half, -half)
	var rb = Vector3(half, -half, half)
	
	return plane

func apply_transformation():
	pass	
	
func create_quad_sphere(subdivisions:int, radius:float):
	var baseCube = BoxMesh.new()
#	baseCube.
	baseCube.subdivide_depth = subdivisions
	baseCube.subdivide_height = subdivisions
	baseCube.subdivide_width = subdivisions

	var mesh = ArrayMesh.new()
	var surfaces = baseCube.get_surface_count()
	
	print("Surfaces: ", surfaces)
	
	for i in range(surfaces):
		var array = baseCube.surface_get_arrays(i)
		var vertices = array[Mesh.ARRAY_VERTEX]
		var uvs = array[Mesh.ARRAY_TEX_UV]
		var normals = array[Mesh.ARRAY_NORMAL]
		var colors = []

		#here we apply transformation
		for j in range(vertices.size()):
#			vertices[j] = vertices[j] + Vector3(0.5, 0.0, 0.5)
			
			vertices[j] = (vertices[j]*(1-Strength) + Strength*vertices[j].normalized()) * radius
			
			uvs[j] = calculate_uv(vertices[j])
			normals[j] = calculate_normals(vertices[j])
			
			colors.append(Color(0, 0.5, 0.5, 1.0))
			
 #			print(c)
#			if i == 0:
#				colors.append(Color(0, 0, c, 1.0)) # Generate a random color for each quad and append it to the colors array
#			else:
#				colors.append(Color(0, 0, 0, 1.0))

		array[Mesh.ARRAY_VERTEX] = vertices
		array[Mesh.ARRAY_TEX_UV] = uvs
		array[Mesh.ARRAY_NORMAL] = normals
		array[Mesh.ARRAY_COLOR] = PackedColorArray(colors)

		mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, array)
	
	
		
	return mesh

#------------------------------------------

func calculate_uv(vertex):
	var theta = atan2(vertex.z, vertex.x) # azimuthal angle
	var phi = acos(vertex.y / Radius) # polar angle

	var u = theta / (2.0 * PI) + 0.5
	var v = phi / PI

	return Vector2(u, v)

func calculate_normals(vertex):
	return vertex.normalized()

