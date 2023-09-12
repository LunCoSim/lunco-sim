extends Node3D

var missions = preload("res://content/datasets/missions.csv")
var landings = []
#print(missions.records)

var pointer: Node3D
	
# Called when the node enters the scene tree for the first time.
func _ready():
	var r = $Moon.Radius
	
#	for i in missions.records:
#		print(i["Mission type"]=="landing")
	
	landings = missions.records.filter(func(i): return i["Mission type"]=="landing")
	landings = landings.filter(func(i): return i["Landing location"]!="tbd")
	
	pointer = add_sphere(r, 0, 0, Color(0, 1, 0))
#	pointer.hide()
	
	add_sphere(r, 90, 0, Color(1, 0, 0)) # North Pole
	add_sphere(r, -90, 0, Color(0, 0, 1)) #South pole
	
	var count := 0
	for i in landings:
		count += 1

		
		var lat = float(i["latitude"])
		var lon = float(i["longitude"])

		var color = Color(abs(sin(count)), abs(cos(count)), abs(cos(0.3*count)))

		add_sphere(r, lat, lon, color)


func add_sphere(r, lat, lon, color=Color(1, 1, 1)):
	var sphere : Node3D = preload("res://apps/future_lunar_missions/sphere.tscn").instantiate()
	
	var lunar_radius = $Moon.Radius
	
	sphere.mesh.radius = 0.5*lunar_radius/10
	sphere.mesh.height = lunar_radius/10
	sphere.translate(spherical_to_cartesian(r, lat, lon))
	
	var material = StandardMaterial3D.new()
	
	material.albedo_color = color
	
	sphere.mesh.surface_set_material(0, material)

	$Moon.add_child(sphere)
	return sphere
	
static func spherical_to_cartesian(r, phi_degrees, theta_degrees) -> Vector3:
	var location := Vector3.ZERO
	
	var phi = deg_to_rad(phi_degrees) - 3.14/2
	var theta = deg_to_rad(theta_degrees)
	
	location.z = -r*sin(phi)*cos(theta)
	location.x = -r*sin(phi)*sin(theta)
	location.y = r*cos(phi)
	
	return location

# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass


func _on_texture_rect_gui_input(event):
	
	var txt_rct: Control = $Control/TextureRect
	
	var size = txt_rct.size

	if event is InputEventMouseButton:
		if event.is_pressed():
			
			# Adjusting 
			
			var scale_adjustment = Vector2(180, -90)
			
			var pos: Vector2 = 2*(event.position/size - Vector2(0.5, 0.5))*scale_adjustment
			
			add_sphere($Moon.Radius, pos.y, pos.x)
			print("Clicked at: ", pos)
			
		else:
			print("Unclick at: ", event.position)
	elif event is InputEventMouseMotion:
		var scale_adjustment = Vector2(180, -90)
			
		var pos: Vector2 = 2*(event.position/size - Vector2(0.5, 0.5))*scale_adjustment
		
		pointer.position = spherical_to_cartesian($Moon.Radius, pos.y, pos.x)
