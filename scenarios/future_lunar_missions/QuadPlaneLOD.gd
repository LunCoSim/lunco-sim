@tool
class_name QuadPlaneLOD
extends Node3D

@export var Subdivisions := 1 : set = set_subdivisions

@export var Normal: = Vector3(0, 0, 1) : set = set_normal
@export var Center: = Vector3(0, 0, 1) : set = set_center
@export	var Size := Vector2(1, 1) : set = set_size

@export var LodLevel := 1 : set = set_lod_level

var plane: QuadPlane

## Godot class functions
func _ready():
	_rebuild()

## Setters

func set_subdivisions(_subdivisions):
	Subdivisions = _subdivisions
	_rebuild()
	
func set_lod_level(_lod_level):
	print("set_lod_level")
	LodLevel = _lod_level
	if plane:
		plane.show_lod_level(LodLevel)

func set_normal(_normal):
	Normal = _normal
	_rebuild()
	
func set_center(_center):
	Center = _center
	_rebuild()
	
func set_size(_size):
	Size = _size
	_rebuild()
	
## Rebuilding 
func _rebuild():
	if plane:
		plane.remove_from_parent(self)
		
	plane = QuadPlane.new(Normal, Center, Size, Subdivisions)	
	plane.add_to_parent(self)
	plane.show_lod_level(LodLevel)


#-----------------------------------
	
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
	
	var _planes = []
	
	var mesh: MeshInstance3D
	
	func _init(normal: Vector3, center:Vector3, size: Vector2, subdivisions: int, radius:=1.0, color:=Color(1, 0, 0), level:int=0, Strength:=0.0):
		print("Subdivisions: ", subdivisions)
		_normal = normal
		_center = center
		_size = size
		
		_radius = radius
		_color = color
		
		_level = level
		
		_strength = Strength
	
		_gen_mesh()
				
		if subdivisions > 0:
			_tl = QuadPlane.new(normal, center - Vector3(size.x/4, size.y/4, 1) * Vector3(normal.y + normal.z, normal.x + normal.z, normal.x + normal.y), size/2, subdivisions-1, radius, color/2, level+1)
			_tr = QuadPlane.new(normal, center + Vector3(size.x/4, size.y/4, 1) * Vector3(-normal.y + normal.z, normal.x - normal.z, -normal.x + normal.y), size/2, subdivisions-1, radius, color/3, level+1)
			_bl = QuadPlane.new(normal, center - Vector3(size.x/4, size.y/4, 1) * Vector3(-normal.y - normal.z, -normal.x - normal.z, normal.x - normal.y), size/2, subdivisions-1, radius, color/4, level+1)
			_br = QuadPlane.new(normal, center + Vector3(size.x/4, size.y/4, 1) * Vector3(normal.y - normal.z, -normal.x + normal.z, normal.x - normal.y), size/2, subdivisions-1, radius, color/5, level+1)	
			_planes = [_tl, _tr, _bl, _br]
		
	## Generate visual mesh represenation		
	func _gen_mesh():	
		mesh = MeshInstance3D.new()
			
		var plane = ArrayMesh.new()
		
		var baseCube = PlaneMesh.new()
			
		if abs(_normal.x) > 0:
			baseCube.orientation = PlaneMesh.FACE_X
		elif abs(_normal.y) > 0:
			baseCube.orientation = PlaneMesh.FACE_Y
		elif abs(_normal.z) > 0:
			baseCube.orientation = PlaneMesh.FACE_Z
			
		baseCube.size = _size
			
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

			_color = Color(randf(), randf(), randf())
			# here we apply transformation
			for j in range(vertices.size()):
				vertices[j] = vertices[j] + 0.5 * abs(_normal) + _center  # Ensure that center is applied to vertices
				
				if _normal.x <0:
					vertices[j] = vertices[j].rotated(Vector3(0, 1, 0), PI)
				elif _normal.y <0:
					vertices[j] = vertices[j].rotated(Vector3(0, 0, 1), PI)
				elif _normal.z <0:
					vertices[j] = vertices[j].rotated(Vector3(1, 0, 0), PI)
					
				vertices[j] = (vertices[j]*(1-_strength) + _strength*vertices[j].normalized()) * _radius
					
				uvs[j] = calculate_uv(vertices[j], _radius)
				normals[j] = vertices[j].normalized() #As it's a sphere just normalizing
				
					
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

	
	func calculate_uv(vertex, radius):
		var theta = atan2(vertex.z, vertex.x) # azimuthal angle
		var phi = acos(vertex.y / radius) # polar angle

		var u = theta / (2.0 * PI) + 0.5
		var v = phi / PI

		return Vector2(u, v)
	
	
	#-----------------
	func show_lod_level(lod_level:=1):
		print("show lod level: ", lod_level, " ", self._level)
		if self._level == lod_level:
			self.mesh.show()
		else:
			self.mesh.hide()
			
		for plane in _planes:
			if plane:
				plane.show_lod_level(lod_level)
					
					
	func add_to_parent(parent: Node3D):
		parent.add_child(self.mesh)
		
		for plane in _planes:
			if plane:
				plane.add_to_parent(parent)
		
	func remove_from_parent(parent: Node3D):
		parent.remove_child(self.mesh)
		
		for plane in _planes:
			if plane:
				plane.remove_from_parent(parent)
				
				
	## Destructor logic	
	func _notification(what):
		if what == NOTIFICATION_PREDELETE:
			# destructor logic
			pass
	
	func _update(camera: Camera3D):
		print("Quad dist: ", self._center.distance_to(camera.position))
