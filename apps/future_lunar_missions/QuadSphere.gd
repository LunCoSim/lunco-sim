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
var sphere: QuadSphere

var _camera

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


func _process(delta):
	sphere._update(_camera)
	
#		print("Distance: ", cam.global_position.length())

#------------------------------------------

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

func _find_camera(from: Array, exclude, cameras: Array):
	for node in from:
		if node != exclude:
			_find_camera(node.get_children(), exclude, cameras)
		if node is Camera3D:
			cameras.append(node)
	
func _grab_camera():
	if Engine.is_editor_hint():
		var a = EditorScript.new()
		var i = a.get_editor_interface()
		
		var cameras = []
		
		_find_camera(i.get_editor_main_screen().get_children(), i.get_edited_scene_root(), cameras)
		
		if cameras.size():
			_camera = cameras[0]
	else:
		_camera = get_viewport().get_camera_3d()
		
func _update_mesh():
	_grab_camera()
	print("Update mesh: ", _camera)
	
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
	
	var _planes:Array = []
	
	func _init(radius: float, subdivisions: int):
		var size = Vector2(1, 1)
		
		Xp = QuadPlane.new(Vector3(1, 0, 0), Vector3(0.5, 0, 0), size, subdivisions, radius, Color(1, 0, 0))
		Xm = QuadPlane.new(Vector3(-1, 0, 0), Vector3(0.5, 0, 0), size, subdivisions, radius, Color(0.5, 0, 0))
		Yp = QuadPlane.new(Vector3(0, 1, 0), Vector3(0, 0.5, 0), size, subdivisions, radius, Color(0, 1, 0))
		Ym = QuadPlane.new(Vector3(0, -1, 0), Vector3(0, 0.5, 0), size, subdivisions, radius, Color(0, 0.5, 0))
		Zp = QuadPlane.new(Vector3(0, 0, 1), Vector3(0, 0, 0.5), size, subdivisions, radius, Color(0, 0, 1))
		Zm = QuadPlane.new(Vector3(0, 0, -1), Vector3(0, 0, 0.5), size, subdivisions, radius, Color(0, 0, 0.5))
		
		_planes = [Xp, Xm, Yp, Ym, Zp, Zm]
		
	func _update(camera: Camera3D):
		if Engine.get_process_frames() % 200 == 0:
			print("Camera position: ", camera.global_position)
			for plane in _planes:
				plane._update(camera)
		
	## Destructor logic	
	func _notification(what):
		if what == NOTIFICATION_PREDELETE:
			# destructor logic
			pass
			

## Class for one of the planes	
class QuadPlane:
	var _normal: Vector3
	var _center: Vector3
	var _size: Vector2
	var _radius
	var _strength
	var _color
	
	var _level
	
	var _tl: QuadPlane #Top Left
	var _tr: QuadPlane
	var _bl: QuadPlane #Bottom Left
	var _br: QuadPlane
	
	var mesh: MeshInstance3D
	
	func _init(normal: Vector3, center:Vector3, size: Vector2, subdivisions: int, radius:=1.0, color:=Color(1, 0, 0), level:int=0, Strength:=1.0):
		print("Subdivisions: ", subdivisions)
		_normal = normal
		_center = center
		_size = size
		
		_radius = radius
		_color = color
		
		_level = level
		
		_strength = Strength
		
		if subdivisions > 0:
			_tl = QuadPlane.new(normal, center, size/2, subdivisions-1, radius, color, level+1)
			_tr = QuadPlane.new(normal, center, size/2, subdivisions-1, radius, color, level+1)
			_bl = QuadPlane.new(normal, center, size/2, subdivisions-1, radius, color, level+1)
			_br = QuadPlane.new(normal, center, size/2, subdivisions-1, radius, color, level+1)
		
		_gen_mesh()
		
		mesh.show()
		if subdivisions == 0:
			mesh.show()

	## Generate visual mesh represenation		
	func _gen_mesh():	
		mesh = MeshInstance3D.new()
		mesh.hide()
		
		var plane = ArrayMesh.new()
	
		var baseCube = PlaneMesh.new()
		
		if abs(_normal.x) > 0:
			baseCube.orientation = PlaneMesh.FACE_X
		elif abs(_normal.y) > 0:
			baseCube.orientation = PlaneMesh.FACE_Y
		elif abs(_normal.z) > 0:
			baseCube.orientation = PlaneMesh.FACE_Z
		
		baseCube.size = Vector2(1, 1)
		
		baseCube.subdivide_depth = 0
		baseCube.subdivide_width = 0
		
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
				vertices[j] = vertices[j]+ 0.5*abs(_normal)
				
				if _normal.x <0:
					vertices[j] = vertices[j].rotated(Vector3(0, 1, 0), PI)
				elif _normal.y <0:
					vertices[j] = vertices[j].rotated(Vector3(0, 0, 1), PI)
				elif _normal.z <0:
					vertices[j] = vertices[j].rotated(Vector3(1, 0, 0), PI)
				
				
				vertices[j] = (vertices[j]*(1-_strength) + _strength*vertices[j].normalized()) * _radius
				
				uvs[j] = QuadSphereNode.calculate_uv(vertices[j], _radius)
				normals[j] = QuadSphereNode.calculate_normals(vertices[j])
				
				colors.append(_color)
				

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
	
	func _update(camera: Camera3D):
		pass
