@tool
class_name QuadSphereNode
extends Node3D

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

#
var sphere

var faces = {
		"X+": {
			"orientation": PlaneMesh.FACE_X,
			"modifier": Vector3(1, 0, 0),
			"color": Color(1, 0, 0)
		},
		"X-": {
			"orientation": PlaneMesh.FACE_X,
			"modifier": Vector3(-1, 0, 0),
			"color": Color(0.5, 0, 0)
		},
		"Y+": {
			"orientation": PlaneMesh.FACE_Y,
			"modifier": Vector3(0, 1, 0),
			"color": Color(0, 1, 0)
		},
		"Y-": {
			"orientation": PlaneMesh.FACE_Y,
			"modifier": Vector3(0, -1, 0),
			"color": Color(0, 0.5, 0)
		},
		"Z+": {
			"orientation": PlaneMesh.FACE_Z,
			"modifier": Vector3(0, 0, 1),
			"color": Color(0, 0, 1)
		},
		"Z-": {
			"orientation": PlaneMesh.FACE_Z,
			"modifier": Vector3(0, 0, -1),
			"color": Color(0, 0, 0.5)
		},
	}
	
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
	if sphere:
		self.remove_child(sphere.Xp.mesh)
		self.remove_child(sphere.Xm.mesh)
		self.remove_child(sphere.Yp.mesh)
		self.remove_child(sphere.Ym.mesh)
		self.remove_child(sphere.Zp.mesh)
		self.remove_child(sphere.Zm.mesh)
		
	sphere = QuadSphere.new(Radius, Subdivisions)
	
	
	self.add_child(sphere.Xp.mesh)
	self.add_child(sphere.Xm.mesh)
	self.add_child(sphere.Yp.mesh)
	self.add_child(sphere.Ym.mesh)
	self.add_child(sphere.Zp.mesh)
	self.add_child(sphere.Zm.mesh)
	
	pass
#	mesh = create_quad_sphere(Subdivisions, Radius)
#
#	var material = StandardMaterial3D.new()
#	material.vertex_color_use_as_albedo = true
#	if mesh != null:
#		for i in range(mesh.get_surface_count()):
#			mesh.surface_set_material(i, material)

#------------------------------------------

func create_quad_sphere(subdivisions:int, radius:float):
	
	var quad_sphere = ArrayMesh.new()
	
	for key in faces.keys():
		
		var face = faces[key]
		var baseCube = PlaneMesh.new()
		baseCube.orientation = face["orientation"]
		baseCube.size = Vector2(1, 1)
		
		baseCube.subdivide_depth = subdivisions
	#	baseCube.subdivide_height = subdivisions #For Quad
		baseCube.subdivide_width = subdivisions
		
	#	baseCube.transla
		
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
				vertices[j] = vertices[j]+ 0.5*abs(face["modifier"])
				
				if face["modifier"].x <0:
					vertices[j] = vertices[j].rotated(Vector3(0, 1, 0), PI)
				elif face["modifier"].y <0:
					vertices[j] = vertices[j].rotated(Vector3(0, 0, 1), PI)
				elif face["modifier"].z <0:
					vertices[j] = vertices[j].rotated(Vector3(1, 0, 0), PI)
				
				
				vertices[j] = (vertices[j]*(1-Strength) + Strength*vertices[j].normalized()) * radius
				
				uvs[j] = calculate_uv(vertices[j], Radius)
				normals[j] = calculate_normals(vertices[j])
				
				colors.append(face["color"])
				

			array[Mesh.ARRAY_VERTEX] = vertices
			array[Mesh.ARRAY_TEX_UV] = uvs
			array[Mesh.ARRAY_NORMAL] = normals
			array[Mesh.ARRAY_COLOR] = PackedColorArray(colors)

			quad_sphere.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, array)
	
	return quad_sphere

#------------------------------------------

static func calculate_uv(vertex, radius):
	var theta = atan2(vertex.z, vertex.x) # azimuthal angle
	var phi = acos(vertex.y / radius) # polar angle

	var u = theta / (2.0 * PI) + 0.5
	var v = phi / PI

	return Vector2(u, v)

static func calculate_normals(vertex):
	return vertex.normalized()

#----------------------------

## Class that contains Sphere data

class QuadSphere:
	## Planes
	var Xp: QuadPlane
	var Xm: QuadPlane
	var Yp: QuadPlane
	var Ym: QuadPlane
	var Zp: QuadPlane
	var Zm: QuadPlane
	
	func _init(radius: float, subdivisions: int):
		Xp = QuadPlane.new(Vector3(1, 0, 0), subdivisions, radius, Color(1, 0, 0))
		Xm = QuadPlane.new(Vector3(-1, 0, 0), subdivisions, radius, Color(0.5, 0, 0))
		Yp = QuadPlane.new(Vector3(0, 1, 0), subdivisions, radius, Color(0, 1, 0))
		Ym = QuadPlane.new(Vector3(0, -1, 0), subdivisions, radius, Color(0, 0.5, 0))
		Zp = QuadPlane.new(Vector3(0, 0, 1), subdivisions, radius, Color(0, 0, 1))
		Zm = QuadPlane.new(Vector3(0, 0, -1), subdivisions, radius, Color(0, 0, 0.5))
	
	## Destructor logic	
	func _notification(what):
		if what == NOTIFICATION_PREDELETE:
			# destructor logic
			pass
			

## Class for one of the planes	
class QuadPlane:
	var _normal: Vector3
	var mesh: MeshInstance3D
	
	func _init(normal: Vector3, subdivisions: int, radius:=1, color:=Color(1, 0, 0), Strength:=1.0):
		_normal = normal
		mesh = MeshInstance3D.new()
		
		var plane = ArrayMesh.new()
	
		var baseCube = PlaneMesh.new()
		
		if abs(normal.x) > 0:
			baseCube.orientation = PlaneMesh.FACE_X
		elif abs(normal.y) > 0:
			baseCube.orientation = PlaneMesh.FACE_Y
		elif abs(normal.z) > 0:
			baseCube.orientation = PlaneMesh.FACE_Z
		
		baseCube.size = Vector2(1, 1)
		
		baseCube.subdivide_depth = subdivisions
		baseCube.subdivide_width = subdivisions
		
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
				vertices[j] = vertices[j]+ 0.5*abs(normal)
				
				if normal.x <0:
					vertices[j] = vertices[j].rotated(Vector3(0, 1, 0), PI)
				elif normal.y <0:
					vertices[j] = vertices[j].rotated(Vector3(0, 0, 1), PI)
				elif normal.z <0:
					vertices[j] = vertices[j].rotated(Vector3(1, 0, 0), PI)
				
				
				vertices[j] = (vertices[j]*(1-Strength) + Strength*vertices[j].normalized()) * radius
				
				uvs[j] = QuadSphereNode.calculate_uv(vertices[j], radius)
				normals[j] = QuadSphereNode.calculate_normals(vertices[j])
				
				colors.append(color)
				

			array[Mesh.ARRAY_VERTEX] = vertices
			array[Mesh.ARRAY_TEX_UV] = uvs
			array[Mesh.ARRAY_NORMAL] = normals
			array[Mesh.ARRAY_COLOR] = PackedColorArray(colors)

			plane.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, array)
		mesh.mesh = plane
		
		var material = StandardMaterial3D.new()
		material.vertex_color_use_as_albedo = true
	
		for i in range(plane.get_surface_count()):
			plane.surface_set_material(i, material)

	## Destructor logic	
	func _notification(what):
		if what == NOTIFICATION_PREDELETE:
			# destructor logic
			pass

## Final leaf	
class QuadLeaf:
	pass
